use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::{Error, Result, anyhow, bail};
use async_trait::async_trait;
use tokio::process::Command;
use tokio::time::sleep;
use tracing::{info, warn};

use orcas_core::{
    AppConfig, AppPaths, CodexThreadAssignmentStatus, DecisionType,
    ORCAS_APP_SERVER_LISTEN_URL_ENV, ORCAS_APP_SERVER_OWNER_KIND_ENV,
    ORCAS_APP_SERVER_OWNER_PID_ENV, ORCAS_APP_SERVER_STARTED_AT_ENV, ORCAS_APP_SERVER_TAG_ENV,
    ORCAS_APP_SERVER_TAG_VALUE, SupervisorProposalEdits, SupervisorProposalRecord,
    SupervisorTurnDecision, SupervisorTurnDecisionKind, SupervisorTurnDecisionStatus,
    ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest, WorkUnitStatus, WorkstreamStatus,
    authority, ipc,
};
use orcasd::{
    OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient, OrcasRuntimeOverrides,
    apply_runtime_overrides,
};

use crate::streaming::{
    ConsoleReporter, OrcasSupervisorStreamingBackend, RetryPolicy, StreamReporter,
    StreamingCommandRunner,
};

pub use orcasd::OrcasRuntimeOverrides as RuntimeOverrides;

const SUPERVISOR_CLI_OPERATOR: &str = "supervisor_cli_operator";
const ORCAS_CLI_NODE_ID: &str = "orcas-cli";

fn print_merge_prep_assessment(assessment: &ipc::TrackedThreadMergePrepAssessment) {
    println!("workspace_scope: merge_prep_assessment");
    println!(
        "merge_prep_assessed_at: {}",
        assessment.assessed_at.to_rfc3339()
    );
    println!("merge_prep_readiness: {:?}", assessment.readiness);
    println!(
        "merge_prep_local_head_commit: {}",
        assessment.local_head_commit.as_deref().unwrap_or("unset")
    );
    println!(
        "merge_prep_worker_reported_head_commit: {}",
        assessment
            .worker_reported_head_commit
            .as_deref()
            .unwrap_or("unset")
    );
    println!(
        "merge_prep_report_id: {}",
        assessment.report_id.as_deref().unwrap_or("unset")
    );
    println!(
        "merge_prep_report_disposition: {}",
        assessment
            .report_disposition
            .map(|value| format!("{value:?}"))
            .unwrap_or_else(|| "unset".to_string())
    );
    if assessment.reasons.is_empty() {
        println!("merge_prep_reasons: none");
    } else {
        for reason in &assessment.reasons {
            println!("merge_prep_reason: {:?}", reason);
        }
    }
}

fn print_landing_authorization(authorization: &orcas_core::LandingAuthorizationRecord) {
    println!("workspace_scope: landing_authorization");
    println!("landing_authorization_id: {}", authorization.id);
    println!("landing_authorization_status: {:?}", authorization.status);
    println!(
        "landing_authorization_tracked_thread_id: {}",
        authorization.tracked_thread_id
    );
    println!(
        "landing_authorization_work_unit_id: {}",
        authorization.work_unit_id
    );
    println!(
        "landing_authorization_worker_id: {}",
        authorization.worker_id.as_deref().unwrap_or("unset")
    );
    println!(
        "landing_authorization_worker_session_id: {}",
        authorization
            .worker_session_id
            .as_deref()
            .unwrap_or("unset")
    );
    println!(
        "landing_authorization_authorized_head_commit: {}",
        authorization.authorized_head_commit
    );
    println!(
        "landing_authorization_landing_target: {}",
        authorization.landing_target
    );
    println!(
        "landing_authorization_linked_merge_prep_operation_id: {}",
        authorization.linked_merge_prep_operation_id
    );
    println!(
        "landing_authorization_merge_prep_assessed_at: {}",
        authorization.merge_prep_assessed_at.to_rfc3339()
    );
    println!(
        "landing_authorization_merge_prep_readiness: {:?}",
        authorization.merge_prep_readiness
    );
    if authorization.merge_prep_reasons.is_empty() {
        println!("landing_authorization_merge_prep_reasons: none");
    } else {
        for reason in &authorization.merge_prep_reasons {
            println!("landing_authorization_merge_prep_reason: {:?}", reason);
        }
    }
    println!(
        "landing_authorization_authorized_by: {}",
        authorization.authorized_by
    );
    println!(
        "landing_authorization_authorized_at: {}",
        authorization.authorized_at.to_rfc3339()
    );
    println!(
        "landing_authorization_updated_at: {}",
        authorization.updated_at.to_rfc3339()
    );
    if let Some(note) = authorization.request_note.as_ref() {
        println!("landing_authorization_request_note: {note}");
    }
    if let Some(report_id) = authorization.merge_prep_report_id.as_ref() {
        println!("landing_authorization_merge_prep_report_id: {report_id}");
    }
    if let Some(disposition) = authorization.merge_prep_report_disposition.as_ref() {
        println!(
            "landing_authorization_merge_prep_report_disposition: {:?}",
            disposition
        );
    }
}

fn print_landing_execution(execution: &orcas_core::LandingExecutionRecord) {
    println!("workspace_scope: landing_execution");
    println!("landing_execution_id: {}", execution.id);
    println!("landing_execution_status: {:?}", execution.status);
    println!(
        "landing_execution_tracked_thread_id: {}",
        execution.tracked_thread_id
    );
    println!("landing_execution_work_unit_id: {}", execution.work_unit_id);
    println!(
        "landing_execution_authorization_id: {}",
        execution.authorization_id
    );
    println!(
        "landing_execution_authorized_head_commit: {}",
        execution.authorized_head_commit
    );
    println!(
        "landing_execution_landing_target: {}",
        execution.landing_target
    );
    println!(
        "landing_execution_worker_id: {}",
        execution.worker_id.as_deref().unwrap_or("unset")
    );
    println!(
        "landing_execution_worker_session_id: {}",
        execution.worker_session_id.as_deref().unwrap_or("unset")
    );
    println!("landing_execution_requested_by: {}", execution.requested_by);
    println!(
        "landing_execution_requested_at: {}",
        execution.requested_at.to_rfc3339()
    );
    println!(
        "landing_execution_updated_at: {}",
        execution.updated_at.to_rfc3339()
    );
    if let Some(note) = execution.request_note.as_ref() {
        println!("landing_execution_request_note: {note}");
    }
    if let Some(report_id) = execution.report_id.as_ref() {
        println!("landing_execution_report_id: {report_id}");
    }
    if let Some(disposition) = execution.report_disposition.as_ref() {
        println!("landing_execution_report_disposition: {:?}", disposition);
    }
    if let Some(status) = execution.result_status.as_ref() {
        println!("landing_execution_result_status: {:?}", status);
    }
    if let Some(head_commit) = execution.attempted_head_commit.as_ref() {
        println!("landing_execution_attempted_head_commit: {head_commit}");
    }
    if let Some(landed_commit) = execution.landed_commit.as_ref() {
        println!("landing_execution_landed_commit: {landed_commit}");
    }
    if let Some(updated) = execution.landing_ref_updated {
        println!("landing_execution_landing_ref_updated: {updated}");
    }
    if let Some(reason) = execution.failure_reason.as_ref() {
        println!("landing_execution_failure_reason: {reason}");
    }
    if let Some(summary) = execution.outcome_summary.as_ref() {
        println!("landing_execution_outcome_summary: {summary}");
    }
    if let Some(notes) = execution.notes.as_ref() {
        println!("landing_execution_notes: {notes}");
    }
}

fn print_prune_workspace_operation(operation: &orcas_core::WorkspaceOperationRecord) {
    println!("workspace_scope: prune_workspace");
    println!("prune_workspace_operation_id: {}", operation.id);
    println!("prune_workspace_operation_kind: {:?}", operation.kind);
    println!("prune_workspace_operation_status: {:?}", operation.status);
    println!(
        "prune_workspace_operation_assignment_id: {}",
        operation.assignment_id
    );
    println!(
        "prune_workspace_operation_work_unit_id: {}",
        operation.work_unit_id
    );
    println!(
        "prune_workspace_operation_linked_landing_execution_id: {}",
        operation
            .linked_landing_execution_id
            .as_deref()
            .unwrap_or("unset")
    );
    println!(
        "prune_workspace_operation_target_worktree_path: {}",
        operation.target_worktree_path.as_deref().unwrap_or("unset")
    );
    println!(
        "prune_workspace_operation_target_branch_name: {}",
        operation.target_branch_name.as_deref().unwrap_or("unset")
    );
    println!(
        "prune_workspace_operation_worker_id: {}",
        operation.worker_id.as_deref().unwrap_or("unset")
    );
    println!(
        "prune_workspace_operation_worker_session_id: {}",
        operation.worker_session_id.as_deref().unwrap_or("unset")
    );
    println!(
        "prune_workspace_operation_requested_by: {}",
        operation.requested_by.as_str()
    );
    println!(
        "prune_workspace_operation_requested_at: {}",
        operation.requested_at.to_rfc3339()
    );
    println!(
        "prune_workspace_operation_updated_at: {}",
        operation.updated_at.to_rfc3339()
    );
    if let Some(note) = operation.request_note.as_ref() {
        println!("prune_workspace_operation_request_note: {note}");
    }
    if let Some(report_id) = operation.report_id.as_ref() {
        println!("prune_workspace_operation_report_id: {report_id}");
    }
    if let Some(disposition) = operation.report_disposition.as_ref() {
        println!(
            "prune_workspace_operation_report_disposition: {:?}",
            disposition
        );
    }
    if let Some(status) = operation.prune_result_status.as_ref() {
        println!("prune_workspace_operation_result_status: {:?}", status);
    }
    if let Some(path) = operation.target_worktree_path.as_ref() {
        println!("prune_workspace_operation_target_worktree_path: {path}");
    }
    if let Some(branch) = operation.target_branch_name.as_ref() {
        println!("prune_workspace_operation_target_branch_name: {branch}");
    }
    if let Some(removed) = operation.worktree_removed {
        println!("prune_workspace_operation_worktree_removed: {removed}");
    }
    if let Some(removed) = operation.branch_removed {
        println!("prune_workspace_operation_branch_removed: {removed}");
    }
    if let Some(reason) = operation.refusal_reason.as_ref() {
        println!("prune_workspace_operation_refusal_reason: {reason}");
    }
    if let Some(reason) = operation.failure_reason.as_ref() {
        println!("prune_workspace_operation_failure_reason: {reason}");
    }
    if let Some(notes) = operation.prune_notes.as_ref() {
        println!("prune_workspace_operation_notes: {notes}");
    }
    if let Some(summary) = operation.outcome_summary.as_ref() {
        println!("prune_workspace_operation_outcome_summary: {summary}");
    }
}

fn print_planning_session(session: &orcas_core::PlanningSession) {
    println!("surface: planning_session");
    println!("planning_session_id: {}", session.session_id);
    println!("planning_session_workstream_id: {}", session.workstream_id);
    println!("planning_session_status: {:?}", session.status);
    println!("planning_session_thread_id: {}", session.planning_thread_id);
    println!(
        "planning_session_base_plan_id: {}",
        session
            .base_plan_id
            .as_ref()
            .map(|plan_id| plan_id.to_string())
            .unwrap_or_else(|| "unset".to_string())
    );
    println!(
        "planning_session_base_plan_version: {}",
        session
            .base_plan_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "unset".to_string())
    );
    println!(
        "planning_session_research_assignment_id: {}",
        session.research_assignment_id.as_deref().unwrap_or("unset")
    );
    println!(
        "planning_session_research_report_id: {}",
        session.research_report_id.as_deref().unwrap_or("unset")
    );
    println!(
        "planning_session_draft_revision_proposal_id: {}",
        session
            .draft_revision_proposal_id
            .as_ref()
            .map(|proposal_id| proposal_id.to_string())
            .unwrap_or_else(|| "unset".to_string())
    );
    println!(
        "planning_session_approved_plan_id: {}",
        session
            .approved_plan_id
            .as_ref()
            .map(|plan_id| plan_id.to_string())
            .unwrap_or_else(|| "unset".to_string())
    );
    println!(
        "planning_session_approved_plan_version: {}",
        session
            .approved_plan_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "unset".to_string())
    );
    println!("planning_session_created_at: {}", session.created_at);
    println!("planning_session_created_by: {}", session.created_by);
    println!("planning_session_updated_at: {}", session.updated_at);
    println!("planning_session_updated_by: {}", session.updated_by);
    if let Some(note) = session.request_note.as_ref() {
        println!("planning_session_request_note: {note}");
    }
    if let Some(reviewed_at) = session.reviewed_at.as_ref() {
        println!("planning_session_reviewed_at: {}", reviewed_at);
    }
    if let Some(reviewed_by) = session.reviewed_by.as_ref() {
        println!("planning_session_reviewed_by: {reviewed_by}");
    }
    if let Some(review_note) = session.review_note.as_ref() {
        println!("planning_session_review_note: {review_note}");
    }
    if let Some(superseded_by_session_id) = session.superseded_by_session_id.as_ref() {
        println!(
            "planning_session_superseded_by_session_id: {}",
            superseded_by_session_id
        );
    }
    let summary = &session.latest_structured_summary;
    println!("planning_session_summary_objective: {}", summary.objective);
    println!(
        "planning_session_summary_research_status: {:?}",
        summary.research_status
    );
    println!(
        "planning_session_summary_ready_for_review: {}",
        summary.ready_for_review
    );
    println!(
        "planning_session_summary_draft_plan_summary: {}",
        summary.draft_plan_summary.as_deref().unwrap_or("unset")
    );
    if summary.requirements.is_empty() {
        println!("planning_session_summary_requirements: none");
    } else {
        for requirement in &summary.requirements {
            println!("planning_session_requirement: {requirement}");
        }
    }
    if summary.constraints.is_empty() {
        println!("planning_session_summary_constraints: none");
    } else {
        for constraint in &summary.constraints {
            println!("planning_session_constraint: {constraint}");
        }
    }
    if summary.non_goals.is_empty() {
        println!("planning_session_summary_non_goals: none");
    } else {
        for non_goal in &summary.non_goals {
            println!("planning_session_non_goal: {non_goal}");
        }
    }
    if summary.open_questions.is_empty() {
        println!("planning_session_summary_open_questions: none");
    } else {
        for question in &summary.open_questions {
            println!("planning_session_open_question: {question}");
        }
    }
}

fn print_planning_revision_proposal(proposal: &orcas_core::planning::PlanRevisionProposal) {
    println!("planning_revision_proposal_id: {}", proposal.proposal_id);
    println!(
        "planning_revision_proposal_workstream_id: {}",
        proposal.workstream_id
    );
    println!(
        "planning_revision_proposal_base_plan_id: {}",
        proposal.base_plan_id
    );
    println!(
        "planning_revision_proposal_base_plan_version: {}",
        proposal.base_plan_version
    );
    println!("planning_revision_proposal_status: {:?}", proposal.status);
    println!(
        "planning_revision_proposal_created_at: {}",
        proposal.created_at
    );
    println!(
        "planning_revision_proposal_created_by: {}",
        proposal.created_by
    );
    println!(
        "planning_revision_proposal_rationale: {}",
        proposal.rationale
    );
    println!(
        "planning_revision_proposal_expected_benefit: {}",
        proposal.expected_benefit
    );
    if proposal.tradeoffs.is_empty() {
        println!("planning_revision_proposal_tradeoffs: none");
    } else {
        for tradeoff in &proposal.tradeoffs {
            println!("planning_revision_proposal_tradeoff: {tradeoff}");
        }
    }
    if proposal.ops.is_empty() {
        println!("planning_revision_proposal_ops: none");
    } else {
        println!("planning_revision_proposal_ops: {}", proposal.ops.len());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalArtifactExportFormat {
    Json,
    Markdown,
}

#[async_trait]
trait SupervisorCodexBackend {
    async fn codex_assignment_list(
        &self,
        params: &ipc::CodexAssignmentListRequest,
    ) -> Result<ipc::CodexAssignmentListResponse>;

    async fn supervisor_decision_list(
        &self,
        params: &ipc::SupervisorDecisionListRequest,
    ) -> Result<ipc::SupervisorDecisionListResponse>;

    async fn supervisor_decision_get(
        &self,
        params: &ipc::SupervisorDecisionGetRequest,
    ) -> Result<ipc::SupervisorDecisionGetResponse>;

    async fn supervisor_decision_propose_steer(
        &self,
        params: &ipc::SupervisorDecisionProposeSteerRequest,
    ) -> Result<ipc::SupervisorDecisionProposeSteerResponse>;

    async fn supervisor_decision_replace_pending_steer(
        &self,
        params: &ipc::SupervisorDecisionReplacePendingSteerRequest,
    ) -> Result<ipc::SupervisorDecisionReplacePendingSteerResponse>;

    async fn supervisor_decision_record_no_action(
        &self,
        params: &ipc::SupervisorDecisionRecordNoActionRequest,
    ) -> Result<ipc::SupervisorDecisionRecordNoActionResponse>;

    async fn supervisor_decision_manual_refresh(
        &self,
        params: &ipc::SupervisorDecisionManualRefreshRequest,
    ) -> Result<ipc::SupervisorDecisionManualRefreshResponse>;

    async fn supervisor_decision_approve_and_send(
        &self,
        params: &ipc::SupervisorDecisionApproveAndSendRequest,
    ) -> Result<ipc::SupervisorDecisionApproveAndSendResponse>;

    async fn supervisor_decision_reject(
        &self,
        params: &ipc::SupervisorDecisionRejectRequest,
    ) -> Result<ipc::SupervisorDecisionRejectResponse>;
}

#[async_trait]
impl SupervisorCodexBackend for OrcasIpcClient {
    async fn codex_assignment_list(
        &self,
        params: &ipc::CodexAssignmentListRequest,
    ) -> Result<ipc::CodexAssignmentListResponse> {
        OrcasIpcClient::codex_assignment_list(self, params)
            .await
            .map_err(Into::into)
    }

    async fn supervisor_decision_list(
        &self,
        params: &ipc::SupervisorDecisionListRequest,
    ) -> Result<ipc::SupervisorDecisionListResponse> {
        OrcasIpcClient::supervisor_decision_list(self, params)
            .await
            .map_err(Into::into)
    }

    async fn supervisor_decision_get(
        &self,
        params: &ipc::SupervisorDecisionGetRequest,
    ) -> Result<ipc::SupervisorDecisionGetResponse> {
        OrcasIpcClient::supervisor_decision_get(self, params)
            .await
            .map_err(Into::into)
    }

    async fn supervisor_decision_propose_steer(
        &self,
        params: &ipc::SupervisorDecisionProposeSteerRequest,
    ) -> Result<ipc::SupervisorDecisionProposeSteerResponse> {
        OrcasIpcClient::supervisor_decision_propose_steer(self, params)
            .await
            .map_err(Into::into)
    }

    async fn supervisor_decision_replace_pending_steer(
        &self,
        params: &ipc::SupervisorDecisionReplacePendingSteerRequest,
    ) -> Result<ipc::SupervisorDecisionReplacePendingSteerResponse> {
        OrcasIpcClient::supervisor_decision_replace_pending_steer(self, params)
            .await
            .map_err(Into::into)
    }

    async fn supervisor_decision_record_no_action(
        &self,
        params: &ipc::SupervisorDecisionRecordNoActionRequest,
    ) -> Result<ipc::SupervisorDecisionRecordNoActionResponse> {
        OrcasIpcClient::supervisor_decision_record_no_action(self, params)
            .await
            .map_err(Into::into)
    }

    async fn supervisor_decision_manual_refresh(
        &self,
        params: &ipc::SupervisorDecisionManualRefreshRequest,
    ) -> Result<ipc::SupervisorDecisionManualRefreshResponse> {
        OrcasIpcClient::supervisor_decision_manual_refresh(self, params)
            .await
            .map_err(Into::into)
    }

    async fn supervisor_decision_approve_and_send(
        &self,
        params: &ipc::SupervisorDecisionApproveAndSendRequest,
    ) -> Result<ipc::SupervisorDecisionApproveAndSendResponse> {
        OrcasIpcClient::supervisor_decision_approve_and_send(self, params)
            .await
            .map_err(Into::into)
    }

    async fn supervisor_decision_reject(
        &self,
        params: &ipc::SupervisorDecisionRejectRequest,
    ) -> Result<ipc::SupervisorDecisionRejectResponse> {
        OrcasIpcClient::supervisor_decision_reject(self, params)
            .await
            .map_err(Into::into)
    }
}

pub struct SupervisorService {
    pub paths: AppPaths,
    pub config: AppConfig,
    daemon: OrcasDaemonProcessManager,
    overrides: OrcasRuntimeOverrides,
}

#[derive(Debug, Clone)]
struct LocalCodexAppServer {
    endpoint: String,
    listen_address: String,
    pid: u32,
    parent_pid: Option<u32>,
    process_name: String,
    command_line: String,
    managed: bool,
    owner_kind: Option<String>,
    owner_pid: Option<u32>,
    owner_pid_running: Option<bool>,
    owner_listen_url: Option<String>,
    owner_started_at: Option<String>,
    reap_hint: String,
}

impl SupervisorService {
    pub async fn load(overrides: &RuntimeOverrides) -> Result<Self> {
        let paths = AppPaths::discover()?;
        paths.ensure().await?;
        let mut config = AppConfig::write_default_if_missing(&paths).await?;
        apply_runtime_overrides(&mut config, overrides);
        let daemon = OrcasDaemonProcessManager::new(paths.clone(), overrides.clone());

        Ok(Self {
            paths,
            config,
            daemon,
            overrides: overrides.clone(),
        })
    }

    pub async fn doctor(&self) -> Result<()> {
        let daemon_status = self.daemon.status().await?;
        println!("config: {}", self.paths.config_file.display());
        println!("state: {}", self.paths.state_file.display());
        println!("state_db: {}", self.paths.state_db_file.display());
        println!("runtime_dir: {}", self.paths.runtime_dir.display());
        println!("socket: {}", daemon_status.socket_path.display());
        println!("metadata: {}", daemon_status.metadata_path.display());
        println!("daemon_running: {}", daemon_status.running);
        println!("daemon_log: {}", daemon_status.log_path.display());
        println!("codex_bin: {}", self.config.codex.binary_path.display());
        println!("codex_endpoint: {}", self.config.codex.listen_url);
        println!("connection_mode: {:?}", self.config.codex.connection_mode);
        Ok(())
    }

    pub async fn daemon_status(&self) -> Result<()> {
        let socket_status = self.daemon.status().await?;
        println!("socket: {}", socket_status.socket_path.display());
        println!("metadata: {}", socket_status.metadata_path.display());
        println!("running: {}", socket_status.running);
        println!("socket_exists: {}", socket_status.socket_exists);
        println!("socket_responsive: {}", socket_status.socket_responsive);
        println!("pid_running: {}", socket_status.pid_running);
        if let Some(pid) = socket_status.socket_owner_pid {
            println!("socket_owner_pid: {pid}");
        }
        println!("stale_socket: {}", socket_status.stale_socket);
        println!("stale_metadata: {}", socket_status.stale_metadata);
        println!("log_file: {}", socket_status.log_path.display());
        if let Some(expected) = socket_status.expected_binary.as_ref() {
            println!("expected_binary: {}", expected.binary_path);
            println!("expected_version: {}", expected.version);
            println!("expected_fingerprint: {}", expected.build_fingerprint);
        }
        if let Some(matches) = socket_status.binary_matches_expected {
            println!("binary_matches_expected: {matches}");
        }
        if let Some(runtime) = socket_status.runtime_metadata.as_ref() {
            println!("daemon_pid: {}", runtime.pid);
            println!("daemon_started_at: {}", runtime.started_at);
            println!("daemon_version: {}", runtime.version);
            println!("daemon_fingerprint: {}", runtime.build_fingerprint);
            println!("daemon_binary: {}", runtime.binary_path);
            if let Some(git_commit) = runtime.git_commit.as_ref() {
                println!("daemon_git_commit: {git_commit}");
            }
        } else if socket_status.running {
            println!("daemon_runtime: legacy daemon without runtime metadata");
        }
        if let Some(status) = socket_status.daemon_status.as_ref() {
            println!("codex_endpoint: {}", status.codex_endpoint);
            println!("codex_binary: {}", status.codex_binary_path);
            println!("upstream_status: {}", status.upstream.status);
            if let Some(detail) = status.upstream.detail.as_ref() {
                println!("upstream_detail: {detail}");
            }
            println!("client_count: {}", status.client_count);
            println!("known_threads: {}", status.known_threads);
        }
        Ok(())
    }

    pub async fn session_active(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.session_get_active().await?;
        println!(
            "active_thread_id: {}",
            response.session.active_thread_id.as_deref().unwrap_or("-")
        );
        println!("active_turns: {}", response.session.active_turns.len());
        for turn in response.session.active_turns {
            println!(
                "{}\t{}\t{}\t{}",
                turn.thread_id, turn.turn_id, turn.status, turn.updated_at
            );
        }
        Ok(())
    }

    pub async fn daemon_discover_app_servers(&self) -> Result<()> {
        let servers = Self::discover_local_codex_app_servers().await?;
        if servers.is_empty() {
            println!("no local codex app-server listeners discovered");
            return Ok(());
        }
        for (index, server) in servers.iter().enumerate() {
            if index > 0 {
                println!();
            }
            Self::print_discovered_app_server(server);
        }
        Ok(())
    }

    pub async fn daemon_reap_app_servers(
        &self,
        apply: bool,
        all_tagged: bool,
        include_untagged: bool,
        target_pids: &[u32],
    ) -> Result<()> {
        let servers = Self::discover_local_codex_app_servers().await?;
        let explicit_targets = !target_pids.is_empty();
        let selected = servers
            .into_iter()
            .filter(|server| {
                if explicit_targets {
                    target_pids.contains(&server.pid)
                } else if all_tagged {
                    server.managed
                } else {
                    server.managed && server.owner_pid_running != Some(true)
                }
            })
            .filter(|server| include_untagged || server.managed)
            .collect::<Vec<_>>();

        let selection_mode = if explicit_targets {
            "explicit-pid-selection"
        } else if all_tagged {
            "all-tagged"
        } else {
            "tagged-owner-dead"
        };

        println!("mode: {}", if apply { "apply" } else { "dry-run" });
        println!("selection: {selection_mode}");
        println!("matched: {}", selected.len());

        if selected.is_empty() {
            println!("no app-servers matched reap selection");
            return Ok(());
        }

        for server in &selected {
            println!();
            Self::print_discovered_app_server(server);
            if !apply {
                println!("action: would send SIGTERM");
            }
        }

        if !apply {
            println!();
            println!("dry_run: rerun with --apply to send SIGTERM");
            if explicit_targets && !include_untagged {
                println!(
                    "note: add --include-untagged to target manually selected untagged listeners"
                );
            }
            return Ok(());
        }

        for server in selected {
            let status = Command::new("kill")
                .args(["-TERM", &server.pid.to_string()])
                .status()
                .await
                .map_err(|error| anyhow!("failed to run `kill -TERM {}`: {error}", server.pid))?;
            if !status.success() {
                bail!("`kill -TERM {}` failed with status {}", server.pid, status);
            }
            println!("reaped_pid: {}", server.pid);
        }
        Ok(())
    }

    pub async fn daemon_start(&self, force: bool) -> Result<()> {
        let launch = if force || self.overrides.force_spawn {
            OrcasDaemonLaunch::Always
        } else {
            OrcasDaemonLaunch::IfNeeded
        };
        let socket_status = self.daemon.ensure_running(launch).await?;
        let client = self.connect_client(OrcasDaemonLaunch::Never).await?;
        let status = client.daemon_connect().await?.status;
        println!("socket: {}", socket_status.socket_path.display());
        println!("metadata: {}", socket_status.metadata_path.display());
        println!("running: {}", socket_status.running);
        println!("log_file: {}", socket_status.log_path.display());
        println!("upstream_status: {}", status.upstream.status);
        println!("codex_endpoint: {}", status.codex_endpoint);
        println!("daemon_pid: {}", status.runtime.pid);
        println!("daemon_version: {}", status.runtime.version);
        println!("daemon_fingerprint: {}", status.runtime.build_fingerprint);
        println!("daemon_binary: {}", status.runtime.binary_path);
        Ok(())
    }

    pub async fn daemon_restart(&self) -> Result<()> {
        let socket_status = self.daemon.restart().await?;
        let client = self.connect_client(OrcasDaemonLaunch::Never).await?;
        let status = client.daemon_connect().await?.status;
        println!("socket: {}", socket_status.socket_path.display());
        println!("metadata: {}", socket_status.metadata_path.display());
        println!("running: {}", socket_status.running);
        println!("log_file: {}", socket_status.log_path.display());
        println!("upstream_status: {}", status.upstream.status);
        println!("codex_endpoint: {}", status.codex_endpoint);
        println!("daemon_pid: {}", status.runtime.pid);
        println!("daemon_version: {}", status.runtime.version);
        println!("daemon_fingerprint: {}", status.runtime.build_fingerprint);
        println!("daemon_binary: {}", status.runtime.binary_path);
        Ok(())
    }

    pub async fn daemon_stop(&self) -> Result<()> {
        let before = self.daemon.status().await?;
        let after = self.daemon.stop().await?;
        println!("socket: {}", before.socket_path.display());
        println!("metadata: {}", before.metadata_path.display());
        println!("running: {}", after.running);
        println!("socket_exists: {}", after.socket_exists);
        println!("stale_socket: {}", after.stale_socket);
        println!("stale_metadata: {}", after.stale_metadata);
        if before.running {
            println!("stopped: true");
        } else if before.stale_socket || before.stale_metadata {
            println!("cleaned_stale_runtime: true");
        } else {
            println!("daemon_already_stopped: true");
        }
        Ok(())
    }

    pub async fn models_list(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.models_list().await?;
        for model in response.data {
            println!(
                "{}\t{}\thidden={}\tdefault={}",
                model.id, model.display_name, model.hidden, model.is_default
            );
        }
        Ok(())
    }

    pub async fn threads_list(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.threads_list_scoped().await?;
        Self::print_thread_list(response.data);
        Ok(())
    }

    pub async fn threads_list_loaded(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.threads_list_loaded().await?;
        Self::print_thread_list(response.data);
        Ok(())
    }

    fn print_thread_list(threads: Vec<ipc::ThreadSummary>) {
        for thread in threads {
            println!(
                "{}\t{}\t{}\t{}\tloaded={:?}\tmonitor={:?}\tin_flight={}\tactive_turn={}\t{}\t{}",
                thread.id,
                thread.status,
                thread.model_provider,
                thread.scope,
                thread.loaded_status,
                thread.monitor_state,
                thread.turn_in_flight,
                thread.active_turn_id.as_deref().unwrap_or("-"),
                thread
                    .recent_output
                    .clone()
                    .unwrap_or_else(|| thread.preview.replace('\n', " ")),
                thread.recent_event.unwrap_or_default()
            );
        }
    }

    pub async fn thread_read(&self, thread_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .thread_read(&ThreadReadRequest {
                thread_id: thread_id.to_string(),
                include_turns: true,
            })
            .await?;
        println!("thread: {}", response.thread.summary.id);
        println!("status: {}", response.thread.summary.status);
        println!("scope: {}", response.thread.summary.scope);
        println!("cwd: {}", response.thread.summary.cwd);
        println!("preview: {}", response.thread.summary.preview);
        if let Some(snippet) = response.thread.summary.recent_output.as_ref() {
            println!("recent_output: {snippet}");
        }
        if let Some(event) = response.thread.summary.recent_event.as_ref() {
            println!("recent_event: {event}");
        }
        println!("turn_in_flight: {}", response.thread.summary.turn_in_flight);
        println!("turns: {}", response.thread.turns.len());
        Ok(())
    }

    pub async fn turns_list_active(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.turns_list_active().await?;
        if response.turns.is_empty() {
            println!("no active attachable turns");
            return Ok(());
        }

        for turn in response.turns {
            println!(
                "{}\t{}\t{}\tattachable={}\tlive_stream={}\t{}\t{}",
                turn.thread_id,
                turn.turn_id,
                format!("{:?}", turn.lifecycle).to_ascii_lowercase(),
                turn.attachable,
                turn.live_stream,
                turn.recent_output.unwrap_or_default(),
                turn.recent_event.unwrap_or_default()
            );
        }
        Ok(())
    }

    pub async fn turns_recent(&self, thread_id: &str, limit: usize) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .turns_recent(&ipc::TurnsRecentRequest {
                thread_id: thread_id.to_string(),
                limit,
            })
            .await?;
        println!("thread_id: {}", response.thread_id);
        println!("turns: {}", response.turns.len());
        for turn in response.turns {
            println!(
                "{}\t{}\titems={}\tstarted={}\tcompleted={}",
                turn.id,
                turn.status,
                turn.items.len(),
                turn.started_at
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| "-".to_string()),
                turn.completed_at
                    .map(|value| value.to_rfc3339())
                    .unwrap_or_else(|| "-".to_string())
            );
            if let Some(summary) = Self::turn_recent_text(&turn) {
                println!("summary: {summary}");
            }
        }
        Ok(())
    }

    pub async fn turn_get(&self, thread_id: &str, turn_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .turn_attach(&orcas_core::ipc::TurnAttachRequest {
                thread_id: thread_id.to_string(),
                turn_id: turn_id.to_string(),
            })
            .await?;

        println!("thread_id: {thread_id}");
        println!("turn_id: {turn_id}");
        println!("attached: {}", response.attached);
        if let Some(reason) = response.reason.as_ref() {
            println!("attach_reason: {reason}");
        }

        if let Some(turn) = response.turn {
            println!(
                "lifecycle: {}",
                format!("{:?}", turn.lifecycle).to_ascii_lowercase()
            );
            println!("status: {}", turn.status);
            println!("attachable: {}", turn.attachable);
            println!("live_stream: {}", turn.live_stream);
            println!("terminal: {}", turn.terminal);
            println!("updated_at: {}", turn.updated_at);
            if let Some(output) = turn.recent_output.as_ref() {
                println!("recent_output: {output}");
            }
            if let Some(event) = turn.recent_event.as_ref() {
                println!("recent_event: {event}");
            }
            if let Some(error) = turn.error_message.as_ref() {
                println!("error_message: {error}");
            }
        } else {
            println!("turn: not found");
        }

        Ok(())
    }

    pub async fn workstream_create(
        &self,
        title: String,
        objective: String,
        priority: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_workstream_create(&ipc::AuthorityWorkstreamCreateRequest {
                command: authority::CreateWorkstream {
                    metadata: Self::authority_command_metadata(),
                    workstream_id: authority::WorkstreamId::new(),
                    title,
                    objective,
                    status: WorkstreamStatus::Active,
                    priority: priority.unwrap_or_else(|| "medium".to_string()),
                },
            })
            .await?;
        println!("surface: authority");
        println!("workstream_id: {}", response.workstream.id);
        println!("revision: {}", response.workstream.revision.get());
        println!("status: {:?}", response.workstream.status);
        Ok(())
    }

    pub async fn workstream_edit(
        &self,
        workstream_id: &str,
        title: Option<String>,
        objective: Option<String>,
        status: Option<WorkstreamStatus>,
        priority: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let existing = client
            .authority_workstream_get(&ipc::AuthorityWorkstreamGetRequest {
                workstream_id: authority::WorkstreamId::parse(workstream_id.to_string())?,
            })
            .await?;
        let patch = authority::WorkstreamPatch {
            title,
            objective,
            status,
            priority,
        };
        if patch.is_empty() {
            bail!("supply at least one workstream field to edit");
        }
        let response = client
            .authority_workstream_edit(&ipc::AuthorityWorkstreamEditRequest {
                command: authority::EditWorkstream {
                    metadata: Self::authority_command_metadata(),
                    workstream_id: existing.workstream.id,
                    expected_revision: existing.workstream.revision,
                    changes: patch,
                },
            })
            .await?;
        println!("surface: authority");
        println!("workstream_id: {}", response.workstream.id);
        println!("revision: {}", response.workstream.revision.get());
        println!("status: {:?}", response.workstream.status);
        Ok(())
    }

    pub async fn workstream_delete(&self, workstream_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let workstream_id = authority::WorkstreamId::parse(workstream_id.to_string())?;
        let delete_plan = client
            .authority_delete_plan(&ipc::AuthorityDeletePlanRequest {
                target: authority::DeleteTarget::Workstream {
                    workstream_id: workstream_id.clone(),
                },
            })
            .await?
            .delete_plan;
        let response = client
            .authority_workstream_delete(&ipc::AuthorityWorkstreamDeleteRequest {
                command: authority::DeleteWorkstream {
                    metadata: Self::authority_command_metadata(),
                    workstream_id,
                    expected_revision: delete_plan.expected_revision,
                    delete_token: delete_plan.confirmation_token,
                },
            })
            .await?;
        println!("surface: authority");
        println!("workstream_id: {}", response.workstream.id);
        println!("revision: {}", response.workstream.revision.get());
        println!("deleted: {}", response.workstream.deleted_at.is_some());
        Ok(())
    }

    pub async fn workstream_list(&self) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_workstream_list(&ipc::AuthorityWorkstreamListRequest::default())
            .await?;
        for workstream in response.workstreams {
            println!(
                "{}\trev={}\t{:?}\t{}\t{}",
                workstream.id,
                workstream.revision.get(),
                workstream.status,
                workstream.priority,
                workstream.title
            );
        }
        Ok(())
    }

    pub async fn events_recent(&self, limit: usize) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client.state_get().await?;
        let total = response.snapshot.recent_events.len();
        let start = total.saturating_sub(limit);
        let events = &response.snapshot.recent_events[start..];
        println!("daemon_recent_events: {}", total);
        if events.is_empty() {
            println!("no recent daemon events");
            return Ok(());
        }
        for event in events {
            Self::print_event_summary(event);
        }
        Ok(())
    }

    pub async fn events_watch(&self, include_snapshot: bool, count: Option<usize>) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let (mut subscription, snapshot) = client.subscribe_events(include_snapshot).await?;
        if let Some(snapshot) = snapshot {
            println!("snapshot_threads: {}", snapshot.threads.len());
            println!(
                "snapshot_active_thread_id: {}",
                snapshot.session.active_thread_id.as_deref().unwrap_or("-")
            );
            println!("snapshot_recent_events: {}", snapshot.recent_events.len());
            for event in snapshot.recent_events {
                Self::print_event_summary(&event);
            }
        }

        let mut seen = 0usize;
        loop {
            tokio::select! {
                event = subscription.recv() => {
                    match event {
                        Ok(envelope) => {
                            Self::print_daemon_event(&envelope);
                            seen += 1;
                            if let Some(limit) = count
                                && seen >= limit
                            {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            println!("event_watch_warning: lagged_by={skipped}");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            println!("event_watch_closed: true");
                            break;
                        }
                    }
                }
                signal = tokio::signal::ctrl_c() => {
                    if signal.is_ok() {
                        println!("event_watch_interrupted: true");
                    }
                    break;
                }
            }
        }

        Ok(())
    }

    pub async fn workstream_get(&self, workstream_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_workstream_get(&ipc::AuthorityWorkstreamGetRequest {
                workstream_id: authority::WorkstreamId::parse(workstream_id.to_string())?,
            })
            .await?;
        println!("surface: authority");
        println!("workstream_id: {}", response.workstream.id);
        println!("title: {}", response.workstream.title);
        println!("objective: {}", response.workstream.objective);
        println!("status: {:?}", response.workstream.status);
        println!("priority: {}", response.workstream.priority);
        println!("revision: {}", response.workstream.revision.get());
        println!("origin_node_id: {}", response.workstream.origin_node_id);
        println!("work_units: {}", response.work_units.len());
        for work_unit in response.work_units {
            println!(
                "work_unit\t{}\trev={}\t{:?}\t{}",
                work_unit.id,
                work_unit.revision.get(),
                work_unit.status,
                work_unit.title
            );
        }
        Ok(())
    }

    pub async fn workunit_create(
        &self,
        workstream_id: &str,
        title: String,
        task_statement: String,
        dependencies: Vec<String>,
    ) -> Result<()> {
        if !dependencies.is_empty() {
            bail!(
                "authority-backed workunit create does not accept legacy collaboration dependencies"
            );
        }
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_workunit_create(&ipc::AuthorityWorkunitCreateRequest {
                command: authority::CreateWorkUnit {
                    metadata: Self::authority_command_metadata(),
                    work_unit_id: authority::WorkUnitId::new(),
                    workstream_id: authority::WorkstreamId::parse(workstream_id.to_string())?,
                    title,
                    task_statement,
                    status: WorkUnitStatus::Ready,
                },
            })
            .await?;
        println!("surface: authority");
        println!("work_unit_id: {}", response.work_unit.id);
        println!("revision: {}", response.work_unit.revision.get());
        println!("status: {:?}", response.work_unit.status);
        Ok(())
    }

    pub async fn workunit_edit(
        &self,
        work_unit_id: &str,
        title: Option<String>,
        task_statement: Option<String>,
        status: Option<WorkUnitStatus>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let existing = client
            .authority_workunit_get(&ipc::AuthorityWorkunitGetRequest {
                work_unit_id: authority::WorkUnitId::parse(work_unit_id.to_string())?,
            })
            .await?;
        let patch = authority::WorkUnitPatch {
            title,
            task_statement,
            status,
        };
        if patch.is_empty() {
            bail!("supply at least one work unit field to edit");
        }
        let response = client
            .authority_workunit_edit(&ipc::AuthorityWorkunitEditRequest {
                command: authority::EditWorkUnit {
                    metadata: Self::authority_command_metadata(),
                    work_unit_id: existing.work_unit.id,
                    expected_revision: existing.work_unit.revision,
                    changes: patch,
                },
            })
            .await?;
        println!("surface: authority");
        println!("work_unit_id: {}", response.work_unit.id);
        println!("revision: {}", response.work_unit.revision.get());
        println!("status: {:?}", response.work_unit.status);
        Ok(())
    }

    pub async fn workunit_delete(&self, work_unit_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let work_unit_id = authority::WorkUnitId::parse(work_unit_id.to_string())?;
        let delete_plan = client
            .authority_delete_plan(&ipc::AuthorityDeletePlanRequest {
                target: authority::DeleteTarget::WorkUnit {
                    work_unit_id: work_unit_id.clone(),
                },
            })
            .await?
            .delete_plan;
        let response = client
            .authority_workunit_delete(&ipc::AuthorityWorkunitDeleteRequest {
                command: authority::DeleteWorkUnit {
                    metadata: Self::authority_command_metadata(),
                    work_unit_id,
                    expected_revision: delete_plan.expected_revision,
                    delete_token: delete_plan.confirmation_token,
                },
            })
            .await?;
        println!("surface: authority");
        println!("work_unit_id: {}", response.work_unit.id);
        println!("revision: {}", response.work_unit.revision.get());
        println!("deleted: {}", response.work_unit.deleted_at.is_some());
        Ok(())
    }

    pub async fn workunit_list(&self, workstream_id: Option<&str>) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_workunit_list(&ipc::AuthorityWorkunitListRequest {
                workstream_id: match workstream_id {
                    Some(workstream_id) => {
                        Some(authority::WorkstreamId::parse(workstream_id.to_string())?)
                    }
                    None => None,
                },
                include_deleted: false,
            })
            .await?;
        for work_unit in response.work_units {
            println!(
                "{}\trev={}\t{:?}\tworkstream={}\t{}",
                work_unit.id,
                work_unit.revision.get(),
                work_unit.status,
                work_unit.workstream_id,
                work_unit.title
            );
        }
        Ok(())
    }

    pub async fn workunit_get(&self, work_unit_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_workunit_get(&ipc::AuthorityWorkunitGetRequest {
                work_unit_id: authority::WorkUnitId::parse(work_unit_id.to_string())?,
            })
            .await?;
        println!("surface: authority");
        println!("work_unit_id: {}", response.work_unit.id);
        println!("workstream_id: {}", response.work_unit.workstream_id);
        println!("title: {}", response.work_unit.title);
        println!("task_statement: {}", response.work_unit.task_statement);
        println!("status: {:?}", response.work_unit.status);
        println!("revision: {}", response.work_unit.revision.get());
        println!("origin_node_id: {}", response.work_unit.origin_node_id);
        println!("tracked_threads: {}", response.tracked_threads.len());
        for tracked_thread in response.tracked_threads {
            println!(
                "tracked_thread\t{}\trev={}\t{:?}\t{:?}\t{}",
                tracked_thread.id,
                tracked_thread.revision.get(),
                tracked_thread.backend_kind,
                tracked_thread.binding_state,
                tracked_thread.title
            );
        }
        Ok(())
    }

    pub async fn tracked_thread_create(
        &self,
        work_unit_id: &str,
        title: String,
        root_dir: String,
        notes: Option<String>,
        upstream_thread_id: Option<String>,
        preferred_model: Option<String>,
        tracked_thread_id: authority::TrackedThreadId,
        workspace: Option<authority::TrackedThreadWorkspace>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_tracked_thread_create(&ipc::AuthorityTrackedThreadCreateRequest {
                command: authority::CreateTrackedThread {
                    metadata: Self::authority_command_metadata(),
                    tracked_thread_id,
                    work_unit_id: authority::WorkUnitId::parse(work_unit_id.to_string())?,
                    title,
                    notes,
                    backend_kind: authority::TrackedThreadBackendKind::Codex,
                    upstream_thread_id,
                    preferred_cwd: Some(root_dir),
                    preferred_model,
                    workspace,
                },
            })
            .await?;
        println!("surface: authority");
        println!("tracked_thread_id: {}", response.tracked_thread.id);
        println!("revision: {}", response.tracked_thread.revision.get());
        println!("binding_state: {:?}", response.tracked_thread.binding_state);
        Ok(())
    }

    pub async fn tracked_thread_edit(
        &self,
        tracked_thread_id: &str,
        title: Option<String>,
        root_dir: Option<String>,
        notes: Option<String>,
        upstream_thread_id: Option<String>,
        binding_state: Option<authority::TrackedThreadBindingState>,
        preferred_model: Option<String>,
        workspace: Option<authority::TrackedThreadWorkspace>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let existing = client
            .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: authority::TrackedThreadId::parse(
                    tracked_thread_id.to_string(),
                )?,
            })
            .await?;
        let patch = authority::TrackedThreadPatch {
            title,
            notes: notes.map(Some),
            backend_kind: None,
            upstream_thread_id: upstream_thread_id.map(Some),
            binding_state,
            preferred_cwd: root_dir.map(Some),
            preferred_model: preferred_model.map(Some),
            last_seen_turn_id: None,
            workspace: workspace.map(Some),
        };
        if patch.is_empty() {
            bail!("supply at least one tracked-thread field to edit");
        }
        let response = client
            .authority_tracked_thread_edit(&ipc::AuthorityTrackedThreadEditRequest {
                command: authority::EditTrackedThread {
                    metadata: Self::authority_command_metadata(),
                    tracked_thread_id: existing.tracked_thread.id,
                    expected_revision: existing.tracked_thread.revision,
                    changes: patch,
                },
            })
            .await?;
        println!("surface: authority");
        println!("tracked_thread_id: {}", response.tracked_thread.id);
        println!("revision: {}", response.tracked_thread.revision.get());
        println!("binding_state: {:?}", response.tracked_thread.binding_state);
        Ok(())
    }

    pub async fn tracked_thread_delete(&self, tracked_thread_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let tracked_thread_id = authority::TrackedThreadId::parse(tracked_thread_id.to_string())?;
        let delete_plan = client
            .authority_delete_plan(&ipc::AuthorityDeletePlanRequest {
                target: authority::DeleteTarget::TrackedThread {
                    tracked_thread_id: tracked_thread_id.clone(),
                },
            })
            .await?
            .delete_plan;
        let response = client
            .authority_tracked_thread_delete(&ipc::AuthorityTrackedThreadDeleteRequest {
                command: authority::DeleteTrackedThread {
                    metadata: Self::authority_command_metadata(),
                    tracked_thread_id,
                    expected_revision: delete_plan.expected_revision,
                    delete_token: delete_plan.confirmation_token,
                },
            })
            .await?;
        println!("surface: authority");
        println!("tracked_thread_id: {}", response.tracked_thread.id);
        println!("revision: {}", response.tracked_thread.revision.get());
        println!("deleted: {}", response.tracked_thread.deleted_at.is_some());
        Ok(())
    }

    pub async fn tracked_thread_list(&self, work_unit_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_tracked_thread_list(&ipc::AuthorityTrackedThreadListRequest {
                work_unit_id: authority::WorkUnitId::parse(work_unit_id.to_string())?,
                include_deleted: false,
            })
            .await?;
        for tracked_thread in response.tracked_threads {
            println!(
                "{}\trev={}\t{:?}\t{:?}\t{}",
                tracked_thread.id,
                tracked_thread.revision.get(),
                tracked_thread.backend_kind,
                tracked_thread.binding_state,
                tracked_thread.title
            );
        }
        Ok(())
    }

    pub async fn tracked_thread_get(&self, tracked_thread_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: authority::TrackedThreadId::parse(
                    tracked_thread_id.to_string(),
                )?,
            })
            .await?;
        println!("surface: authority");
        println!("tracked_thread_id: {}", response.tracked_thread.id);
        println!("work_unit_id: {}", response.tracked_thread.work_unit_id);
        println!("title: {}", response.tracked_thread.title);
        println!("backend_kind: {:?}", response.tracked_thread.backend_kind);
        println!("binding_state: {:?}", response.tracked_thread.binding_state);
        println!("revision: {}", response.tracked_thread.revision.get());
        println!("origin_node_id: {}", response.tracked_thread.origin_node_id);
        if let Some(root_dir) = response.tracked_thread.preferred_cwd.as_ref() {
            println!("preferred_cwd: {root_dir}");
        }
        if let Some(upstream_thread_id) = response.tracked_thread.upstream_thread_id.as_ref() {
            println!("upstream_thread_id: {upstream_thread_id}");
        }
        if let Some(notes) = response.tracked_thread.notes.as_ref() {
            println!("notes: {notes}");
        }
        if let Some(preferred_model) = response.tracked_thread.preferred_model.as_ref() {
            println!("preferred_model: {preferred_model}");
        }
        if let Some(workspace) = response.tracked_thread.workspace.as_ref() {
            println!("workspace_scope: supervisor_intent");
            println!("workspace_repository_root: {}", workspace.repository_root);
            println!("workspace_worktree_path: {}", workspace.worktree_path);
            println!("workspace_branch_name: {}", workspace.branch_name);
            println!("workspace_base_ref: {}", workspace.base_ref);
            println!(
                "workspace_base_commit: {}",
                workspace.base_commit.as_deref().unwrap_or("unset")
            );
            println!("workspace_landing_target: {}", workspace.landing_target);
            println!("workspace_status: {:?}", workspace.status);
            println!(
                "workspace_last_reported_head_commit: {}",
                workspace
                    .last_reported_head_commit
                    .as_deref()
                    .unwrap_or("unset")
            );
        }
        if let Some(inspection) = response.workspace_inspection.as_ref() {
            println!("workspace_scope: daemon_inspection");
            println!(
                "workspace_inspected_at: {}",
                inspection.inspected_at.to_rfc3339()
            );
            println!(
                "workspace_local_repository_root: {}",
                inspection.repository_root
            );
            println!(
                "workspace_local_worktree_path: {}",
                inspection.worktree_path
            );
            println!("workspace_local_exists: {}", inspection.exists);
            println!(
                "workspace_local_is_git_worktree: {}",
                inspection.is_git_worktree
            );
            println!(
                "workspace_local_branch_name: {}",
                inspection.current_branch.as_deref().unwrap_or("unset")
            );
            println!(
                "workspace_local_head_commit: {}",
                inspection.current_head_commit.as_deref().unwrap_or("unset")
            );
            println!(
                "workspace_local_dirty: {}",
                inspection
                    .dirty
                    .map(|dirty| dirty.to_string())
                    .unwrap_or_else(|| "unset".to_string())
            );
            println!(
                "workspace_local_base_ref: {}",
                inspection.base_ref.as_deref().unwrap_or("unset")
            );
            println!(
                "workspace_local_base_commit: {}",
                inspection.base_commit.as_deref().unwrap_or("unset")
            );
            println!(
                "workspace_local_landing_target: {}",
                inspection.landing_target.as_deref().unwrap_or("unset")
            );
            if let Some(comparison) = inspection.base_commit_comparison.as_ref() {
                println!(
                    "workspace_local_base_commit_comparison: reference={} ahead={} behind={}",
                    comparison.reference, comparison.ahead_by, comparison.behind_by
                );
            }
            if let Some(comparison) = inspection.landing_target_comparison.as_ref() {
                println!(
                    "workspace_local_landing_target_comparison: reference={} ahead={} behind={}",
                    comparison.reference, comparison.ahead_by, comparison.behind_by
                );
            }
            if inspection.warnings.is_empty() {
                println!("workspace_local_warnings: none");
            } else {
                for warning in &inspection.warnings {
                    println!("workspace_local_warning: {:?}", warning);
                }
            }
        }
        if let Some(operation) = response.workspace_operation.as_ref() {
            println!("workspace_scope: daemon_operation");
            println!("workspace_operation_id: {}", operation.id);
            println!("workspace_operation_kind: {:?}", operation.kind);
            println!("workspace_operation_status: {:?}", operation.status);
            println!(
                "workspace_operation_assignment_id: {}",
                operation.assignment_id
            );
            println!(
                "workspace_operation_work_unit_id: {}",
                operation.work_unit_id
            );
            println!(
                "workspace_operation_worker_id: {}",
                operation.worker_id.as_deref().unwrap_or("unset")
            );
            println!(
                "workspace_operation_worker_session_id: {}",
                operation.worker_session_id.as_deref().unwrap_or("unset")
            );
            println!(
                "workspace_operation_requested_by: {}",
                operation.requested_by.as_str()
            );
            println!(
                "workspace_operation_requested_at: {}",
                operation.requested_at.to_rfc3339()
            );
            println!(
                "workspace_operation_updated_at: {}",
                operation.updated_at.to_rfc3339()
            );
            if let Some(note) = operation.request_note.as_ref() {
                println!("workspace_operation_request_note: {note}");
            }
            if let Some(report_id) = operation.report_id.as_ref() {
                println!("workspace_operation_report_id: {report_id}");
            }
            if let Some(disposition) = operation.report_disposition.as_ref() {
                println!("workspace_operation_report_disposition: {:?}", disposition);
            }
            if let Some(summary) = operation.outcome_summary.as_ref() {
                println!("workspace_operation_outcome_summary: {summary}");
            }
        }
        if let Some(operation) = response.prune_workspace_operation.as_ref() {
            print_prune_workspace_operation(operation);
        }
        if let Some(assessment) = response.merge_prep_assessment.as_ref() {
            print_merge_prep_assessment(assessment);
        }
        if let Some(execution) = response.landing_execution.as_ref() {
            println!(
                "landing_execution_matches_authorization_basis: {}",
                response
                    .landing_execution_matches_authorization_basis
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unset".to_string())
            );
            print_landing_execution(execution);
        }
        if let Some(authorization) = response.landing_authorization.as_ref() {
            println!(
                "landing_authorization_is_current: {}",
                response
                    .landing_authorization_is_current
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unset".to_string())
            );
            print_landing_authorization(authorization);
        }
        Ok(())
    }

    pub async fn tracked_thread_prepare_workspace(
        &self,
        tracked_thread_id: &str,
        request_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let tracked_thread = client
            .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: authority::TrackedThreadId::parse(
                    tracked_thread_id.to_string(),
                )?,
            })
            .await?
            .tracked_thread;
        let workspace = tracked_thread
            .workspace
            .clone()
            .ok_or_else(|| anyhow!("tracked thread `{tracked_thread_id}` has no workspace"))?;
        let response = client
            .authority_tracked_thread_prepare_workspace(
                &ipc::AuthorityTrackedThreadPrepareWorkspaceRequest {
                    tracked_thread_id: tracked_thread.id.clone(),
                    requested_by: Some(SUPERVISOR_CLI_OPERATOR.to_string()),
                    request_note,
                    model: tracked_thread.preferred_model.clone(),
                    cwd: Some(workspace.repository_root.clone()),
                },
            )
            .await?;
        println!("surface: workspace_operation");
        println!(
            "workspace_operation_kind: {:?}",
            response.workspace_operation.kind
        );
        println!(
            "workspace_operation_status: {:?}",
            response.workspace_operation.status
        );
        println!(
            "workspace_operation_tracked_thread_id: {}",
            response.workspace_operation.tracked_thread_id
        );
        println!(
            "workspace_operation_assignment_id: {}",
            response.assignment.id
        );
        println!("workspace_operation_worker_id: {}", response.worker.id);
        println!(
            "workspace_operation_worker_session_id: {}",
            response.worker_session.id
        );
        println!("workspace_operation_report_id: {}", response.report.id);
        println!(
            "workspace_operation_report_disposition: {:?}",
            response.report.disposition
        );
        println!(
            "workspace_operation_report_summary: {}",
            response.report.summary
        );
        Ok(())
    }

    pub async fn tracked_thread_refresh_workspace(
        &self,
        tracked_thread_id: &str,
        request_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let tracked_thread = client
            .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: authority::TrackedThreadId::parse(
                    tracked_thread_id.to_string(),
                )?,
            })
            .await?
            .tracked_thread;
        let workspace = tracked_thread
            .workspace
            .clone()
            .ok_or_else(|| anyhow!("tracked thread `{tracked_thread_id}` has no workspace"))?;
        let response = client
            .authority_tracked_thread_refresh_workspace(
                &ipc::AuthorityTrackedThreadRefreshWorkspaceRequest {
                    tracked_thread_id: tracked_thread.id.clone(),
                    requested_by: Some(SUPERVISOR_CLI_OPERATOR.to_string()),
                    request_note,
                    model: tracked_thread.preferred_model.clone(),
                    cwd: Some(workspace.repository_root.clone()),
                },
            )
            .await?;
        println!("surface: workspace_operation");
        println!(
            "workspace_operation_kind: {:?}",
            response.workspace_operation.kind
        );
        println!(
            "workspace_operation_status: {:?}",
            response.workspace_operation.status
        );
        println!(
            "workspace_operation_tracked_thread_id: {}",
            response.workspace_operation.tracked_thread_id
        );
        println!(
            "workspace_operation_assignment_id: {}",
            response.assignment.id
        );
        println!("workspace_operation_worker_id: {}", response.worker.id);
        println!(
            "workspace_operation_worker_session_id: {}",
            response.worker_session.id
        );
        println!("workspace_operation_report_id: {}", response.report.id);
        println!(
            "workspace_operation_report_disposition: {:?}",
            response.report.disposition
        );
        println!(
            "workspace_operation_report_summary: {}",
            response.report.summary
        );
        Ok(())
    }

    pub async fn tracked_thread_prune_workspace(
        &self,
        tracked_thread_id: &str,
        request_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let tracked_thread = client
            .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: authority::TrackedThreadId::parse(
                    tracked_thread_id.to_string(),
                )?,
            })
            .await?
            .tracked_thread;
        let workspace = tracked_thread
            .workspace
            .clone()
            .ok_or_else(|| anyhow!("tracked thread `{tracked_thread_id}` has no workspace"))?;
        let response = client
            .authority_tracked_thread_prune_workspace(
                &ipc::AuthorityTrackedThreadPruneWorkspaceRequest {
                    tracked_thread_id: tracked_thread.id.clone(),
                    requested_by: Some(SUPERVISOR_CLI_OPERATOR.to_string()),
                    request_note,
                    model: tracked_thread.preferred_model.clone(),
                    cwd: Some(workspace.repository_root.clone()),
                },
            )
            .await?;
        println!("surface: workspace_operation");
        println!(
            "workspace_operation_kind: {:?}",
            response.workspace_operation.kind
        );
        println!(
            "workspace_operation_status: {:?}",
            response.workspace_operation.status
        );
        println!(
            "workspace_operation_tracked_thread_id: {}",
            response.workspace_operation.tracked_thread_id
        );
        println!(
            "workspace_operation_assignment_id: {}",
            response.assignment.id
        );
        println!("workspace_operation_worker_id: {}", response.worker.id);
        println!(
            "workspace_operation_worker_session_id: {}",
            response.worker_session.id
        );
        println!("workspace_operation_report_id: {}", response.report.id);
        println!(
            "workspace_operation_report_disposition: {:?}",
            response.report.disposition
        );
        println!(
            "workspace_operation_report_summary: {}",
            response.report.summary
        );
        if let Some(prune_result) = response.prune_workspace_result.as_ref() {
            println!("surface: prune_workspace_result");
            println!(
                "prune_workspace_result_tracked_thread_id: {}",
                prune_result
                    .tracked_thread_id
                    .as_ref()
                    .map(|tracked_thread_id| tracked_thread_id.to_string())
                    .unwrap_or_else(|| "unset".to_string())
            );
            println!(
                "prune_workspace_result_worktree_path: {}",
                prune_result.worktree_path
            );
            println!(
                "prune_workspace_result_branch_name: {}",
                prune_result.branch_name.as_deref().unwrap_or("unset")
            );
            println!("prune_workspace_result_status: {:?}", prune_result.status);
            if let Some(value) = prune_result.worktree_removed {
                println!("prune_workspace_result_worktree_removed: {value}");
            }
            if let Some(value) = prune_result.branch_removed {
                println!("prune_workspace_result_branch_removed: {value}");
            }
            if let Some(reason) = prune_result.refusal_reason.as_ref() {
                println!("prune_workspace_result_refusal_reason: {reason}");
            }
            if let Some(reason) = prune_result.failure_reason.as_ref() {
                println!("prune_workspace_result_failure_reason: {reason}");
            }
            if let Some(notes) = prune_result.notes.as_ref() {
                println!("prune_workspace_result_notes: {notes}");
            }
        }
        Ok(())
    }

    pub async fn planning_session_create(
        &self,
        workstream_id: &str,
        planning_thread_id: Option<String>,
        objective: String,
        research_status: orcas_core::PlanningSessionResearchStatus,
        requirements: Vec<String>,
        constraints: Vec<String>,
        non_goals: Vec<String>,
        open_questions: Vec<String>,
        draft_plan_summary: Option<String>,
        ready_for_review: bool,
        created_by: Option<String>,
        request_note: Option<String>,
        model: Option<String>,
        cwd: Option<PathBuf>,
    ) -> Result<()> {
        if ready_for_review {
            bail!(
                "planning session create cannot mark a session ready for review; use plan mark-ready-for-review after creation"
            );
        }
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_create(&ipc::PlanningSessionCreateRequest {
                workstream_id: workstream_id.to_string(),
                planning_thread_id,
                initial_objective: objective,
                research_status,
                requirements,
                constraints,
                non_goals,
                open_questions,
                draft_plan_summary,
                created_by,
                request_note,
                model,
                cwd: cwd.map(|path| path.display().to_string()),
            })
            .await?;
        print_planning_session(&response.session);
        println!(
            "planning_session_create_effect: draft_session_started; readiness must be set later with mark-ready-for-review"
        );
        Ok(())
    }

    pub async fn planning_session_get(&self, session_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_get(&ipc::PlanningSessionGetRequest {
                session_id: session_id.to_string(),
            })
            .await?;
        print_planning_session(&response.session);
        Ok(())
    }

    pub async fn planning_session_list(
        &self,
        workstream_id: Option<String>,
        include_closed: bool,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_list(&ipc::PlanningSessionListRequest {
                workstream_id,
                include_closed,
            })
            .await?;
        if response.sessions.is_empty() {
            println!("no planning sessions");
            return Ok(());
        }
        for session in response.sessions {
            println!(
                "{}\t{:?}\t{}\t{}\t{}",
                session.session_id,
                session.status,
                session.workstream_id,
                session.planning_thread_id,
                session.updated_at
            );
        }
        Ok(())
    }

    pub async fn planning_session_update_summary(
        &self,
        session_id: &str,
        objective: String,
        requirements: Vec<String>,
        constraints: Vec<String>,
        non_goals: Vec<String>,
        open_questions: Vec<String>,
        research_status: orcas_core::PlanningSessionResearchStatus,
        draft_plan_summary: Option<String>,
        ready_for_review: bool,
        updated_by: Option<String>,
        note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_update_summary(&ipc::PlanningSessionUpdateSummaryRequest {
                session_id: session_id.to_string(),
                summary: orcas_core::PlanningSessionStructuredSummary {
                    objective,
                    requirements,
                    constraints,
                    non_goals,
                    open_questions,
                    research_status,
                    draft_plan_summary,
                    ready_for_review,
                },
                updated_by,
                note,
            })
            .await?;
        print_planning_session(&response.session);
        println!(
            "planning_session_update_effect: descriptive_summary_only; use mark-ready-for-review for explicit readiness transition"
        );
        Ok(())
    }

    pub async fn planning_session_request_supervisor_context(
        &self,
        session_id: &str,
        requested_by: Option<String>,
        note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_request_supervisor_context(
                &ipc::PlanningSessionRequestSupervisorContextRequest {
                    session_id: session_id.to_string(),
                    requested_by,
                    note,
                },
            )
            .await?;
        print_planning_session(&response.session);
        Ok(())
    }

    pub async fn planning_session_request_research(
        &self,
        session_id: &str,
        worker_id: &str,
        worker_kind: Option<String>,
        model: Option<String>,
        cwd: Option<PathBuf>,
        requested_by: Option<String>,
        request_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_request_research(&ipc::PlanningSessionRequestResearchRequest {
                session_id: session_id.to_string(),
                worker_id: worker_id.to_string(),
                requested_by,
                request_note,
                worker_kind,
                model,
                cwd: cwd.map(|path| path.display().to_string()),
            })
            .await?;
        print_planning_session(&response.session);
        println!("research_assignment_id: {}", response.assignment.id);
        println!(
            "research_assignment_status: {:?}",
            response.assignment.status
        );
        println!("research_worker_id: {}", response.worker.id);
        println!("research_worker_session_id: {}", response.worker_session.id);
        println!("research_report_id: {}", response.report.id);
        println!(
            "research_report_disposition: {:?}",
            response.report.disposition
        );
        println!("research_report_summary: {}", response.report.summary);
        println!(
            "planning_session_research_effect: bounded_research_turn_requested; repeated requests for this session will be rejected"
        );
        Ok(())
    }

    pub async fn planning_session_mark_ready_for_review(
        &self,
        session_id: &str,
        updated_by: Option<String>,
        note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_mark_ready_for_review(
                &ipc::PlanningSessionMarkReadyForReviewRequest {
                    session_id: session_id.to_string(),
                    updated_by,
                    note,
                },
            )
            .await?;
        print_planning_session(&response.session);
        println!(
            "planning_session_ready_for_review_effect: explicit_readiness_transition; use approve to stage the canonical revision proposal"
        );
        Ok(())
    }

    pub async fn planning_session_abort(
        &self,
        session_id: &str,
        updated_by: Option<String>,
        note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_abort(&ipc::PlanningSessionAbortRequest {
                session_id: session_id.to_string(),
                updated_by,
                note,
            })
            .await?;
        print_planning_session(&response.session);
        Ok(())
    }

    pub async fn planning_session_approve(
        &self,
        session_id: &str,
        approved_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_approve(&ipc::PlanningSessionApproveRequest {
                session_id: session_id.to_string(),
                approved_by,
                review_note,
            })
            .await?;
        print_planning_session(&response.session);
        if let Some(proposal) = response.revision_proposal.as_ref() {
            print_planning_revision_proposal(proposal);
            println!(
                "planning_session_approval_effect: staged_revision_proposal_only; apply it through the existing plan revision approval path"
            );
        }
        Ok(())
    }

    pub async fn planning_session_reject(
        &self,
        session_id: &str,
        rejected_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_reject(&ipc::PlanningSessionRejectRequest {
                session_id: session_id.to_string(),
                rejected_by,
                review_note,
            })
            .await?;
        print_planning_session(&response.session);
        Ok(())
    }

    pub async fn planning_session_supersede(
        &self,
        session_id: &str,
        superseded_by_session_id: Option<String>,
        updated_by: Option<String>,
        note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .planning_session_supersede(&ipc::PlanningSessionSupersedeRequest {
                session_id: session_id.to_string(),
                superseded_by_session_id,
                updated_by,
                note,
            })
            .await?;
        print_planning_session(&response.session);
        Ok(())
    }

    pub async fn tracked_thread_merge_prep(
        &self,
        tracked_thread_id: &str,
        request_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let tracked_thread = client
            .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: authority::TrackedThreadId::parse(
                    tracked_thread_id.to_string(),
                )?,
            })
            .await?
            .tracked_thread;
        let workspace = tracked_thread
            .workspace
            .clone()
            .ok_or_else(|| anyhow!("tracked thread `{tracked_thread_id}` has no workspace"))?;
        let response = client
            .authority_tracked_thread_merge_prep(&ipc::AuthorityTrackedThreadMergePrepRequest {
                tracked_thread_id: tracked_thread.id.clone(),
                requested_by: Some(SUPERVISOR_CLI_OPERATOR.to_string()),
                request_note,
                model: tracked_thread.preferred_model.clone(),
                cwd: Some(workspace.repository_root.clone()),
            })
            .await?;
        println!("surface: workspace_operation");
        println!(
            "workspace_operation_kind: {:?}",
            response.workspace_operation.kind
        );
        println!(
            "workspace_operation_status: {:?}",
            response.workspace_operation.status
        );
        println!(
            "workspace_operation_tracked_thread_id: {}",
            response.workspace_operation.tracked_thread_id
        );
        println!(
            "workspace_operation_assignment_id: {}",
            response.assignment.id
        );
        println!("workspace_operation_worker_id: {}", response.worker.id);
        println!(
            "workspace_operation_worker_session_id: {}",
            response.worker_session.id
        );
        println!("workspace_operation_report_id: {}", response.report.id);
        println!(
            "workspace_operation_report_disposition: {:?}",
            response.report.disposition
        );
        println!(
            "workspace_operation_report_summary: {}",
            response.report.summary
        );
        if let Some(assessment) = response.merge_prep_assessment.as_ref() {
            print_merge_prep_assessment(assessment);
        }
        Ok(())
    }

    pub async fn tracked_thread_authorize_merge(
        &self,
        tracked_thread_id: &str,
        request_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let tracked_thread = client
            .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: authority::TrackedThreadId::parse(
                    tracked_thread_id.to_string(),
                )?,
            })
            .await?
            .tracked_thread;
        let response = client
            .authority_tracked_thread_authorize_merge(
                &ipc::AuthorityTrackedThreadAuthorizeMergeRequest {
                    tracked_thread_id: tracked_thread.id.clone(),
                    authorized_by: Some(SUPERVISOR_CLI_OPERATOR.to_string()),
                    request_note,
                },
            )
            .await?;
        println!("surface: landing_authorization");
        println!(
            "landing_authorization_is_current: {}",
            response
                .landing_authorization_is_current
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unset".to_string())
        );
        print_landing_authorization(&response.landing_authorization);
        if let Some(assessment) = response.merge_prep_assessment.as_ref() {
            print_merge_prep_assessment(assessment);
        }
        if let Some(inspection) = response.workspace_inspection.as_ref() {
            println!("workspace_scope: daemon_inspection");
            println!(
                "workspace_inspected_at: {}",
                inspection.inspected_at.to_rfc3339()
            );
            println!(
                "workspace_local_head_commit: {}",
                inspection.current_head_commit.as_deref().unwrap_or("unset")
            );
        }
        println!("tracked_thread_id: {}", response.tracked_thread.id);
        Ok(())
    }

    pub async fn tracked_thread_execute_landing(
        &self,
        tracked_thread_id: &str,
        request_note: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let tracked_thread = client
            .authority_tracked_thread_get(&ipc::AuthorityTrackedThreadGetRequest {
                tracked_thread_id: authority::TrackedThreadId::parse(
                    tracked_thread_id.to_string(),
                )?,
            })
            .await?
            .tracked_thread;
        let response = client
            .authority_tracked_thread_execute_landing(
                &ipc::AuthorityTrackedThreadExecuteLandingRequest {
                    tracked_thread_id: tracked_thread.id.clone(),
                    authorized_by: Some(SUPERVISOR_CLI_OPERATOR.to_string()),
                    request_note,
                    model: tracked_thread.preferred_model.clone(),
                    cwd: tracked_thread
                        .workspace
                        .as_ref()
                        .map(|workspace| workspace.repository_root.clone()),
                },
            )
            .await?;
        println!("surface: landing_execution");
        println!(
            "landing_execution_matches_authorization_basis: {}",
            response
                .landing_execution_matches_authorization_basis
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unset".to_string())
        );
        print_landing_execution(&response.landing_execution);
        if let Some(authorization) = response.landing_authorization.as_ref() {
            print_landing_authorization(authorization);
            println!(
                "landing_authorization_is_current: {}",
                response
                    .landing_authorization_is_current
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unset".to_string())
            );
        }
        if let Some(assessment) = response.merge_prep_assessment.as_ref() {
            print_merge_prep_assessment(assessment);
        }
        if let Some(inspection) = response.workspace_inspection.as_ref() {
            println!("workspace_scope: daemon_inspection");
            println!(
                "workspace_inspected_at: {}",
                inspection.inspected_at.to_rfc3339()
            );
            println!(
                "workspace_local_head_commit: {}",
                inspection.current_head_commit.as_deref().unwrap_or("unset")
            );
        }
        println!("tracked_thread_id: {}", response.tracked_thread.id);
        Ok(())
    }

    pub async fn assignment_start(
        &self,
        work_unit_id: &str,
        worker_id: &str,
        instructions: Option<String>,
        worker_kind: Option<String>,
        cwd: Option<PathBuf>,
        model: Option<String>,
    ) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .assignment_start(&ipc::AssignmentStartRequest {
                work_unit_id: work_unit_id.to_string(),
                worker_id: worker_id.to_string(),
                worker_kind,
                instructions,
                model,
                cwd: cwd.map(|path| path.display().to_string()),
                plan_id: None,
                plan_version: None,
                plan_item_id: None,
                execution_kind: orcas_core::planning::PlanExecutionKind::DirectExecution,
                alignment_rationale: None,
            })
            .await?;
        println!("assignment_id: {}", response.assignment.id);
        println!("assignment_status: {:?}", response.assignment.status);
        println!("worker_id: {}", response.worker.id);
        println!("worker_session_id: {}", response.worker_session.id);
        if let Some(thread_id) = response.worker_session.thread_id.as_ref() {
            println!("thread_id: {thread_id}");
        }
        println!("report_id: {}", response.report.id);
        println!("report_parse_result: {:?}", response.report.parse_result);
        println!(
            "report_needs_supervisor_review: {}",
            response.report.needs_supervisor_review
        );
        println!("report_disposition: {:?}", response.report.disposition);
        println!("report_summary: {}", response.report.summary);
        Ok(())
    }

    pub async fn assignment_get(&self, assignment_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .assignment_get(&ipc::AssignmentGetRequest {
                assignment_id: assignment_id.to_string(),
            })
            .await?;
        println!("assignment_id: {}", response.assignment.id);
        println!("work_unit_id: {}", response.assignment.work_unit_id);
        println!("worker_id: {}", response.worker.id);
        println!("status: {:?}", response.assignment.status);
        println!("attempt: {}", response.assignment.attempt_number);
        println!("worker_session_id: {}", response.worker_session.id);
        if let Some(report) = response.report.as_ref() {
            println!("report_id: {}", report.id);
            println!("report_parse_result: {:?}", report.parse_result);
            println!(
                "report_needs_supervisor_review: {}",
                report.needs_supervisor_review
            );
        }
        Ok(())
    }

    pub async fn assignment_communication_get(&self, assignment_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .assignment_communication_get(&ipc::AssignmentCommunicationGetRequest {
                assignment_id: assignment_id.to_string(),
            })
            .await?;
        println!("{}", serde_json::to_string_pretty(&response.record)?);
        Ok(())
    }

    pub async fn report_get(&self, report_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .report_get(&ipc::ReportGetRequest {
                report_id: report_id.to_string(),
            })
            .await?;
        println!("report_id: {}", response.report.id);
        println!("work_unit_id: {}", response.report.work_unit_id);
        println!("assignment_id: {}", response.report.assignment_id);
        println!("disposition: {:?}", response.report.disposition);
        println!("parse_result: {:?}", response.report.parse_result);
        println!(
            "needs_supervisor_review: {}",
            response.report.needs_supervisor_review
        );
        println!("confidence: {:?}", response.report.confidence);
        println!("summary: {}", response.report.summary);
        println!("findings: {}", response.report.findings.len());
        println!("blockers: {}", response.report.blockers.len());
        println!("questions: {}", response.report.questions.len());
        println!(
            "recommended_next_actions: {}",
            response.report.recommended_next_actions.len()
        );
        Ok(())
    }

    pub async fn report_list_for_workunit(&self, work_unit_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .report_list_for_workunit(&ipc::ReportListForWorkunitRequest {
                work_unit_id: work_unit_id.to_string(),
            })
            .await?;
        for report in response.reports {
            println!(
                "{}\t{:?}\t{:?}\treview={}\t{}",
                report.id,
                report.disposition,
                report.parse_result,
                report.needs_supervisor_review,
                report.summary
            );
        }
        Ok(())
    }

    pub async fn decision_apply(
        &self,
        work_unit_id: &str,
        report_id: Option<String>,
        decision_type: DecisionType,
        rationale: String,
        instructions: Option<String>,
        worker_id: Option<String>,
        worker_kind: Option<String>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .decision_apply(&ipc::DecisionApplyRequest {
                work_unit_id: work_unit_id.to_string(),
                report_id,
                decision_type,
                rationale,
                instructions,
                worker_id,
                worker_kind,
            })
            .await?;
        println!("decision_id: {}", response.decision.id);
        println!("decision_type: {:?}", response.decision.decision_type);
        println!("work_unit_status: {:?}", response.work_unit.status);
        if let Some(next_assignment) = response.next_assignment.as_ref() {
            println!("next_assignment_id: {}", next_assignment.id);
            println!("next_assignment_status: {:?}", next_assignment.status);
        }
        Ok(())
    }

    pub async fn proposal_create(
        &self,
        work_unit_id: &str,
        source_report_id: Option<String>,
        note: Option<String>,
        requested_by: Option<String>,
        supersede_open: bool,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "create_proposal",
            work_unit_id,
            source_report_id = source_report_id.as_deref().unwrap_or("latest"),
            "starting proposal authoring action"
        );
        let client = match self.daemon_state_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "create_proposal",
                    work_unit_id,
                    source_report_id = source_report_id.as_deref().unwrap_or("latest"),
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "proposal authoring action failed"
                );
                return Err(error);
            }
        };
        let result = client
            .proposal_create(&ipc::ProposalCreateRequest {
                work_unit_id: work_unit_id.to_string(),
                source_report_id,
                requested_by,
                note,
                supersede_open,
            })
            .await;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "create_proposal",
                    work_unit_id,
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "proposal authoring action failed"
                );
                return Err(error.into());
            }
        };
        Self::print_proposal_record(&response.proposal);
        info!(
            surface = "cli",
            action = "create_proposal",
            work_unit_id,
            proposal_id = %response.proposal.id,
            result = "created",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "proposal authoring action completed"
        );
        Ok(())
    }

    pub async fn proposal_get(&self, proposal_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .proposal_get(&ipc::ProposalGetRequest {
                proposal_id: proposal_id.to_string(),
            })
            .await?;
        Self::print_proposal_record(&response.proposal);
        Ok(())
    }

    pub async fn proposal_artifact_summary_get(&self, proposal_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .proposal_artifact_summary_get(&ipc::ProposalArtifactSummaryGetRequest {
                proposal_id: proposal_id.to_string(),
            })
            .await?;
        Self::print_proposal_artifact_summary(&response.summary);
        Ok(())
    }

    pub async fn proposal_artifact_detail_get(&self, proposal_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .proposal_artifact_detail_get(&ipc::ProposalArtifactDetailGetRequest {
                proposal_id: proposal_id.to_string(),
            })
            .await?;
        Self::print_proposal_artifact_detail(&response.detail);
        Ok(())
    }

    pub async fn proposal_artifact_export(
        &self,
        proposal_id: &str,
        format: ProposalArtifactExportFormat,
        output: Option<&Path>,
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .proposal_artifact_export_get(&ipc::ProposalArtifactExportGetRequest {
                proposal_id: proposal_id.to_string(),
            })
            .await?;
        let rendered = match format {
            ProposalArtifactExportFormat::Json => {
                Self::render_proposal_artifact_export_json(&response.export)?
            }
            ProposalArtifactExportFormat::Markdown => {
                Self::render_proposal_artifact_export_markdown(&response.export)?
            }
        };
        if let Some(path) = output {
            fs::write(path, rendered)?;
        } else {
            print!("{rendered}");
            if !rendered.ends_with('\n') {
                println!();
            }
        }
        Ok(())
    }

    pub async fn proposal_list_for_workunit(&self, work_unit_id: &str) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .proposal_list_for_workunit(&ipc::ProposalListForWorkunitRequest {
                work_unit_id: work_unit_id.to_string(),
            })
            .await?;
        if response.proposals.is_empty() {
            println!("no proposals for work unit: {work_unit_id}");
            return Ok(());
        }

        for proposal in response.proposals {
            println!(
                "{}\t{:?}\t{}\t{}\t{}\t{}",
                proposal.id,
                proposal.status,
                proposal
                    .proposed_decision_type
                    .map(|decision| format!("{decision:?}"))
                    .unwrap_or_else(|| "-".to_string()),
                proposal.created_at,
                proposal.reasoner_model,
                proposal.source_report_id
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn proposal_approve(
        &self,
        proposal_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
        decision_type: Option<DecisionType>,
        decision_rationale: Option<String>,
        preferred_worker_id: Option<String>,
        worker_kind: Option<String>,
        objective: Option<String>,
        instructions: Vec<String>,
        acceptance_criteria: Vec<String>,
        stop_conditions: Vec<String>,
        expected_report_fields: Vec<String>,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "approve_proposal",
            proposal_id,
            "starting review action"
        );
        let client = match self.daemon_state_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "approve_proposal",
                    proposal_id,
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        let result = client
            .proposal_approve(&ipc::ProposalApproveRequest {
                proposal_id: proposal_id.to_string(),
                reviewed_by,
                review_note,
                edits: SupervisorProposalEdits {
                    decision_type,
                    decision_rationale,
                    preferred_worker_id,
                    worker_kind,
                    objective,
                    instructions,
                    acceptance_criteria,
                    stop_conditions,
                    expected_report_fields,
                },
            })
            .await;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "approve_proposal",
                    proposal_id,
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error.into());
            }
        };

        Self::print_proposal_record(&response.proposal);
        println!("decision_id: {}", response.decision.id);
        println!("decision_type: {:?}", response.decision.decision_type);
        println!("decision_rationale: {}", response.decision.rationale);
        if let Some(next_assignment) = response.next_assignment.as_ref() {
            println!("next_assignment_id: {}", next_assignment.id);
            println!("next_assignment_status: {:?}", next_assignment.status);
            println!("next_assignment_worker: {}", next_assignment.worker_id);
        } else {
            println!("next_assignment_id:");
        }
        info!(
            surface = "cli",
            action = "approve_proposal",
            proposal_id,
            result = "approved",
            decision_id = %response.decision.id,
            next_assignment_id = response
                .next_assignment
                .as_ref()
                .map(|assignment| assignment.id.as_str())
                .unwrap_or("none"),
            duration_ms = started_at.elapsed().as_millis() as u64,
            "review action completed"
        );
        Ok(())
    }

    pub async fn proposal_reject(
        &self,
        proposal_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "reject_proposal",
            proposal_id,
            "starting review action"
        );
        let client = match self.daemon_state_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "reject_proposal",
                    proposal_id,
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        let result = client
            .proposal_reject(&ipc::ProposalRejectRequest {
                proposal_id: proposal_id.to_string(),
                reviewed_by,
                review_note,
            })
            .await;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "reject_proposal",
                    proposal_id,
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error.into());
            }
        };
        Self::print_proposal_record(&response.proposal);
        info!(
            surface = "cli",
            action = "reject_proposal",
            proposal_id,
            result = "rejected",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "review action completed"
        );
        Ok(())
    }

    pub async fn codex_decision_list(
        &self,
        thread_id: Option<&str>,
        assignment_id: Option<&str>,
        workstream_id: Option<&str>,
        work_unit_id: Option<&str>,
        supervisor_id: Option<&str>,
        status: Option<SupervisorTurnDecisionStatus>,
        kind: Option<SupervisorTurnDecisionKind>,
        include_closed: bool,
        include_superseded: bool,
        actionable_only: bool,
        limit: Option<usize>,
    ) -> Result<()> {
        let client = self.ready_client().await?;
        let decisions = Self::codex_decision_list_with_backend(
            client.as_ref(),
            thread_id,
            assignment_id,
            workstream_id,
            work_unit_id,
            supervisor_id,
            status,
            kind,
            include_closed,
            include_superseded,
            actionable_only,
            limit,
        )
        .await?;

        if decisions.is_empty() {
            if actionable_only {
                println!("no actionable codex supervisor decisions");
            } else {
                println!("no codex supervisor decisions");
            }
            return Ok(());
        }

        for decision in decisions {
            println!(
                "{}",
                Self::format_supervisor_turn_decision_summary(&decision)
            );
        }
        Ok(())
    }

    pub async fn codex_decision_history(
        &self,
        thread_id: Option<&str>,
        assignment_id: Option<&str>,
        include_superseded: bool,
        limit: Option<usize>,
    ) -> Result<()> {
        match (thread_id, assignment_id) {
            (Some(_), Some(_)) => bail!("specify either --thread or --assignment, not both"),
            (None, None) => bail!("history requires --thread or --assignment"),
            _ => {}
        }

        let client = self.ready_client().await?;
        let decisions = Self::codex_decision_list_with_backend(
            client.as_ref(),
            thread_id,
            assignment_id,
            None,
            None,
            None,
            None,
            None,
            true,
            include_superseded,
            false,
            limit,
        )
        .await?;

        if decisions.is_empty() {
            println!("no codex supervisor decision history");
            return Ok(());
        }

        if let Some(thread_id) = thread_id {
            println!("history_for_thread: {thread_id}");
        }
        if let Some(assignment_id) = assignment_id {
            println!("history_for_assignment: {assignment_id}");
        }
        for decision in &decisions {
            println!(
                "{}",
                Self::format_supervisor_turn_decision_summary(decision)
            );
        }
        let chains = Self::format_decision_revision_chains(&decisions);
        if !chains.is_empty() {
            println!("revision_chains:");
            for chain in chains {
                println!("  {chain}");
            }
        }
        Ok(())
    }

    pub async fn codex_decision_get(&self, decision_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
        let decision = Self::codex_decision_get_with_backend(client.as_ref(), decision_id).await?;
        let related = Self::codex_decision_list_with_backend(
            client.as_ref(),
            None,
            Some(&decision.assignment_id),
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            false,
            None,
        )
        .await?;
        Self::print_supervisor_turn_decision(
            &decision,
            related
                .iter()
                .find(|summary| summary.decision_id == decision.decision_id),
            &related,
        );
        Ok(())
    }

    pub async fn codex_decision_propose_steer(
        &self,
        thread_id: &str,
        proposed_text: &str,
        requested_by: Option<String>,
        rationale_note: Option<String>,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "propose_steer",
            thread_id,
            "starting proposal authoring action"
        );
        let client = match self.ready_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "propose_steer",
                    thread_id,
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "proposal authoring action failed"
                );
                return Err(error);
            }
        };
        let result = Self::codex_decision_propose_steer_with_backend(
            client.as_ref(),
            thread_id,
            proposed_text,
            requested_by,
            rationale_note,
        )
        .await;
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "propose_steer",
                    thread_id,
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "proposal authoring action failed"
                );
                return Err(error);
            }
        };
        Self::print_supervisor_turn_decision(&decision, None, &[]);
        info!(
            surface = "cli",
            action = "propose_steer",
            thread_id,
            decision_id = %decision.decision_id,
            assignment_id = %decision.assignment_id,
            result = "created",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "proposal authoring action completed"
        );
        Ok(())
    }

    pub async fn codex_decision_replace_pending_steer(
        &self,
        decision_id: &str,
        proposed_text: &str,
        requested_by: Option<String>,
        rationale_note: Option<String>,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "replace_pending_steer",
            decision_id,
            "starting proposal authoring action"
        );
        let client = match self.ready_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "replace_pending_steer",
                    decision_id,
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "proposal authoring action failed"
                );
                return Err(error);
            }
        };
        let result = Self::codex_decision_replace_pending_steer_with_backend(
            client.as_ref(),
            decision_id,
            proposed_text,
            requested_by,
            rationale_note,
        )
        .await;
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "replace_pending_steer",
                    decision_id,
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "proposal authoring action failed"
                );
                return Err(error);
            }
        };
        Self::print_supervisor_turn_decision(&decision, None, &[]);
        info!(
            surface = "cli",
            action = "replace_pending_steer",
            decision_id = %decision.decision_id,
            assignment_id = %decision.assignment_id,
            result = "created",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "proposal authoring action completed"
        );
        Ok(())
    }

    pub async fn codex_decision_record_no_action(
        &self,
        decision_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "record_no_action",
            decision_id,
            "starting review action"
        );
        let client = match self.ready_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "record_no_action",
                    decision_id,
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        let result = Self::codex_decision_record_no_action_with_backend(
            client.as_ref(),
            decision_id,
            reviewed_by,
            review_note,
        )
        .await;
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "record_no_action",
                    decision_id,
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        Self::print_supervisor_turn_decision(&decision, None, &[]);
        info!(
            surface = "cli",
            action = "record_no_action",
            decision_id = %decision.decision_id,
            assignment_id = %decision.assignment_id,
            result = "completed",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "review action completed"
        );
        Ok(())
    }

    pub async fn codex_decision_manual_refresh(
        &self,
        thread_id: Option<&str>,
        assignment_id: Option<&str>,
        requested_by: Option<String>,
        rationale_note: Option<String>,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "manual_refresh",
            assignment_id = assignment_id.unwrap_or("-"),
            thread_id = thread_id.unwrap_or("-"),
            "starting review action"
        );
        let client = match self.ready_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "manual_refresh",
                    assignment_id = assignment_id.unwrap_or("-"),
                    thread_id = thread_id.unwrap_or("-"),
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        let result = Self::codex_decision_manual_refresh_with_backend(
            client.as_ref(),
            thread_id,
            assignment_id,
            requested_by,
            rationale_note,
        )
        .await;
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "manual_refresh",
                    assignment_id = assignment_id.unwrap_or("-"),
                    thread_id = thread_id.unwrap_or("-"),
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        Self::print_supervisor_turn_decision(&decision, None, &[]);
        info!(
            surface = "cli",
            action = "manual_refresh",
            decision_id = %decision.decision_id,
            assignment_id = %decision.assignment_id,
            result = "completed",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "review action completed"
        );
        Ok(())
    }

    pub async fn codex_decision_approve_and_send(
        &self,
        decision_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "approve_and_send",
            decision_id,
            "starting review action"
        );
        let client = match self.ready_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "approve_and_send",
                    decision_id,
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        let result = Self::codex_decision_approve_and_send_with_backend(
            client.as_ref(),
            decision_id,
            reviewed_by,
            review_note,
        )
        .await;
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "approve_and_send",
                    decision_id,
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        Self::print_supervisor_turn_decision(&decision, None, &[]);
        info!(
            surface = "cli",
            action = "approve_and_send",
            decision_id = %decision.decision_id,
            assignment_id = %decision.assignment_id,
            result = "completed",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "review action completed"
        );
        Ok(())
    }

    pub async fn codex_decision_reject(
        &self,
        decision_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<()> {
        let started_at = Instant::now();
        info!(
            surface = "cli",
            action = "reject_decision",
            decision_id,
            "starting review action"
        );
        let client = match self.ready_client().await {
            Ok(client) => client,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "reject_decision",
                    decision_id,
                    result = "failed",
                    reason = "backend_client_unavailable",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        let result = Self::codex_decision_reject_with_backend(
            client.as_ref(),
            decision_id,
            reviewed_by,
            review_note,
        )
        .await;
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => {
                warn!(
                    surface = "cli",
                    action = "reject_decision",
                    decision_id,
                    result = "failed",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "review action failed"
                );
                return Err(error);
            }
        };
        Self::print_supervisor_turn_decision(&decision, None, &[]);
        info!(
            surface = "cli",
            action = "reject_decision",
            decision_id = %decision.decision_id,
            assignment_id = %decision.assignment_id,
            result = "completed",
            duration_ms = started_at.elapsed().as_millis() as u64,
            "review action completed"
        );
        Ok(())
    }

    pub async fn thread_start(
        &self,
        cwd: Option<PathBuf>,
        model: Option<String>,
        ephemeral: bool,
    ) -> Result<String> {
        let client = self.ready_client().await?;
        let response = client
            .thread_start(&ThreadStartRequest {
                cwd: cwd
                    .or_else(|| self.config.defaults.cwd.clone())
                    .map(|path| path.display().to_string()),
                model: model.or_else(|| self.config.defaults.model.clone()),
                ephemeral,
            })
            .await?;
        println!("thread_id: {}", response.thread.id);
        Ok(response.thread.id)
    }

    pub async fn thread_resume(
        &self,
        thread_id: &str,
        cwd: Option<PathBuf>,
        model: Option<String>,
    ) -> Result<String> {
        let client = self.ready_client().await?;
        let response = client
            .thread_resume(&ThreadResumeRequest {
                thread_id: thread_id.to_string(),
                cwd: cwd
                    .or_else(|| self.config.defaults.cwd.clone())
                    .map(|path| path.display().to_string()),
                model: model.or_else(|| self.config.defaults.model.clone()),
            })
            .await?;
        println!("thread_id: {}", response.thread.id);
        Ok(response.thread.id)
    }

    pub async fn prompt(&self, thread_id: &str, text: &str) -> Result<String> {
        let mut reporter = ConsoleReporter;
        self.resume_thread_for_streaming(thread_id, &mut reporter)
            .await?;
        self.run_streaming_turn(thread_id, text, &mut reporter)
            .await
    }

    pub async fn quickstart(
        &self,
        cwd: Option<PathBuf>,
        model: Option<String>,
        text: &str,
    ) -> Result<()> {
        let mut reporter = ConsoleReporter;
        let cwd = cwd.or_else(|| self.config.defaults.cwd.clone());
        let model = model.or_else(|| self.config.defaults.model.clone());
        let thread_id = self
            .start_thread_for_streaming(cwd, model, &mut reporter)
            .await?;
        let final_text = self
            .run_streaming_turn(&thread_id, text, &mut reporter)
            .await?;
        println!("\nthread_id: {thread_id}");
        println!("final_text_len: {}", final_text.len());
        Ok(())
    }

    async fn resume_thread_for_streaming(
        &self,
        thread_id: &str,
        reporter: &mut dyn StreamReporter,
    ) -> Result<()> {
        let retry_policy = RetryPolicy::default();
        let request = ThreadResumeRequest {
            thread_id: thread_id.to_string(),
            cwd: self
                .config
                .defaults
                .cwd
                .clone()
                .map(|path| path.display().to_string()),
            model: self.config.defaults.model.clone(),
        };
        let mut delay = retry_policy.base_delay;

        for attempt in 1..=retry_policy.max_attempts {
            let client = self.ready_client().await?;
            match client.thread_resume(&request).await {
                Ok(_) => return Ok(()),
                Err(error) => {
                    if attempt == retry_policy.max_attempts {
                        reporter.status(
                            "[daemon connection was lost while resuming the thread; resume could not be confirmed]",
                        );
                        return Err(error.into());
                    }

                    reporter.status(&format!(
                        "[daemon connection was lost while resuming the thread; retrying ({attempt}/{})]",
                        retry_policy.max_attempts
                    ));
                    sleep(delay).await;
                    delay = (delay * 2).min(retry_policy.max_delay);
                }
            }
        }

        Ok(())
    }

    async fn start_thread_for_streaming(
        &self,
        cwd: Option<PathBuf>,
        model: Option<String>,
        reporter: &mut dyn StreamReporter,
    ) -> Result<String> {
        let client = self.ready_client().await?;
        let thread = match client
            .thread_start(&ThreadStartRequest {
                cwd: cwd.map(|path| path.display().to_string()),
                model,
                ephemeral: false,
            })
            .await
        {
            Ok(thread) => thread,
            Err(error) => {
                reporter.status(
                    "[daemon connection was lost while creating the thread; thread creation could not be confirmed]",
                );
                return Err(error.into());
            }
        };
        Ok(thread.thread.id)
    }

    async fn run_streaming_turn(
        &self,
        thread_id: &str,
        text: &str,
        reporter: &mut dyn StreamReporter,
    ) -> Result<String> {
        let backend =
            OrcasSupervisorStreamingBackend::new(self.paths.clone(), &self.config, &self.overrides);
        let runner = StreamingCommandRunner::new(backend, RetryPolicy::default());
        let outcome = runner.run_turn(thread_id, text, reporter).await?;
        if matches!(
            outcome.state,
            crate::streaming::StreamOutcomeState::Interrupted
        ) {
            println!("[stream state: interrupted]");
        }
        Ok(outcome.final_text)
    }

    async fn codex_decision_propose_steer_with_backend<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        thread_id: &str,
        proposed_text: &str,
        requested_by: Option<String>,
        rationale_note: Option<String>,
    ) -> Result<SupervisorTurnDecision> {
        let assignment =
            Self::resolve_active_codex_assignment_for_thread(backend, thread_id).await?;
        let proposed_text = Self::require_non_empty_text("steer text", proposed_text)?;
        let response = backend
            .supervisor_decision_propose_steer(&ipc::SupervisorDecisionProposeSteerRequest {
                assignment_id: assignment.assignment_id,
                requested_by: Some(Self::normalize_actor(requested_by, SUPERVISOR_CLI_OPERATOR)),
                proposed_text: Some(proposed_text),
                rationale_note,
            })
            .await?;
        Ok(response.decision)
    }

    async fn codex_decision_list_with_backend<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        thread_id: Option<&str>,
        assignment_id: Option<&str>,
        workstream_id: Option<&str>,
        work_unit_id: Option<&str>,
        supervisor_id: Option<&str>,
        status: Option<SupervisorTurnDecisionStatus>,
        kind: Option<SupervisorTurnDecisionKind>,
        include_closed: bool,
        include_superseded: bool,
        actionable_only: bool,
        limit: Option<usize>,
    ) -> Result<Vec<ipc::SupervisorTurnDecisionSummary>> {
        let response = backend
            .supervisor_decision_list(&ipc::SupervisorDecisionListRequest {
                assignment_id: assignment_id.map(ToOwned::to_owned),
                codex_thread_id: thread_id.map(ToOwned::to_owned),
                workstream_id: workstream_id.map(ToOwned::to_owned),
                work_unit_id: work_unit_id.map(ToOwned::to_owned),
                supervisor_id: supervisor_id.map(ToOwned::to_owned),
                status,
                kind,
                include_closed,
                include_superseded,
                actionable_only,
                limit,
            })
            .await?;
        Ok(response.decisions)
    }

    async fn codex_decision_get_with_backend<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        decision_id: &str,
    ) -> Result<SupervisorTurnDecision> {
        let response = backend
            .supervisor_decision_get(&ipc::SupervisorDecisionGetRequest {
                decision_id: decision_id.to_string(),
            })
            .await?;
        Ok(response.decision)
    }

    async fn codex_decision_record_no_action_with_backend<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        decision_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<SupervisorTurnDecision> {
        let response = backend
            .supervisor_decision_record_no_action(&ipc::SupervisorDecisionRecordNoActionRequest {
                decision_id: decision_id.to_string(),
                reviewed_by: Some(Self::normalize_actor(reviewed_by, SUPERVISOR_CLI_OPERATOR)),
                review_note,
            })
            .await?;
        Ok(response.decision)
    }

    async fn codex_decision_replace_pending_steer_with_backend<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        decision_id: &str,
        proposed_text: &str,
        requested_by: Option<String>,
        rationale_note: Option<String>,
    ) -> Result<SupervisorTurnDecision> {
        let proposed_text = Self::require_non_empty_text("steer text", proposed_text)?;
        let response = backend
            .supervisor_decision_replace_pending_steer(
                &ipc::SupervisorDecisionReplacePendingSteerRequest {
                    decision_id: decision_id.to_string(),
                    requested_by: Some(Self::normalize_actor(
                        requested_by,
                        SUPERVISOR_CLI_OPERATOR,
                    )),
                    proposed_text,
                    rationale_note,
                },
            )
            .await?;
        Ok(response.decision)
    }

    async fn codex_decision_manual_refresh_with_backend<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        thread_id: Option<&str>,
        assignment_id: Option<&str>,
        requested_by: Option<String>,
        rationale_note: Option<String>,
    ) -> Result<SupervisorTurnDecision> {
        let assignment_id = match (thread_id, assignment_id) {
            (Some(thread_id), None) => {
                Self::resolve_active_codex_assignment_for_thread(backend, thread_id)
                    .await?
                    .assignment_id
            }
            (None, Some(assignment_id)) => assignment_id.to_string(),
            (Some(_), Some(_)) => bail!("specify either --thread or --assignment, not both"),
            (None, None) => bail!("manual refresh requires --thread or --assignment"),
        };
        let response = backend
            .supervisor_decision_manual_refresh(&ipc::SupervisorDecisionManualRefreshRequest {
                assignment_id,
                requested_by: Some(Self::normalize_actor(requested_by, SUPERVISOR_CLI_OPERATOR)),
                rationale_note,
            })
            .await?;
        Ok(response.decision)
    }

    async fn codex_decision_approve_and_send_with_backend<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        decision_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<SupervisorTurnDecision> {
        let response = backend
            .supervisor_decision_approve_and_send(&ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision_id.to_string(),
                reviewed_by: Some(Self::normalize_actor(reviewed_by, SUPERVISOR_CLI_OPERATOR)),
                review_note,
            })
            .await?;
        Ok(response.decision)
    }

    async fn codex_decision_reject_with_backend<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        decision_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<SupervisorTurnDecision> {
        let response = backend
            .supervisor_decision_reject(&ipc::SupervisorDecisionRejectRequest {
                decision_id: decision_id.to_string(),
                reviewed_by: Some(Self::normalize_actor(reviewed_by, SUPERVISOR_CLI_OPERATOR)),
                review_note,
            })
            .await?;
        Ok(response.decision)
    }

    async fn resolve_active_codex_assignment_for_thread<B: SupervisorCodexBackend + Sync>(
        backend: &B,
        thread_id: &str,
    ) -> Result<ipc::CodexThreadAssignmentSummary> {
        let response = backend
            .codex_assignment_list(&ipc::CodexAssignmentListRequest {
                codex_thread_id: Some(thread_id.to_string()),
                workstream_id: None,
                work_unit_id: None,
                include_inactive: false,
            })
            .await?;
        let mut assignments = response.assignments.into_iter().filter(|assignment| {
            assignment.codex_thread_id == thread_id
                && assignment.status == CodexThreadAssignmentStatus::Active
                && assignment.active
        });
        let assignment = assignments
            .next()
            .ok_or_else(|| anyhow!("no active Codex assignment for thread `{thread_id}`"))?;
        if assignments.next().is_some() {
            bail!("multiple active Codex assignments found for thread `{thread_id}`");
        }
        Ok(assignment)
    }

    fn require_non_empty_text(label: &str, text: &str) -> Result<String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            bail!("{label} must not be empty");
        }
        Ok(trimmed.to_string())
    }

    fn normalize_actor(actor: Option<String>, fallback: &str) -> String {
        actor
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| fallback.to_string())
    }

    fn authority_command_metadata() -> authority::CommandMetadata {
        authority::CommandMetadata::new(
            authority::OriginNodeId::parse(ORCAS_CLI_NODE_ID).expect("static cli origin node id"),
            authority::CommandActor::parse(SUPERVISOR_CLI_OPERATOR).expect("static cli actor"),
        )
    }

    fn print_supervisor_turn_decision(
        decision: &SupervisorTurnDecision,
        summary: Option<&ipc::SupervisorTurnDecisionSummary>,
        related: &[ipc::SupervisorTurnDecisionSummary],
    ) {
        for line in Self::format_supervisor_turn_decision(decision, summary, related) {
            println!("{line}");
        }
    }

    fn format_supervisor_turn_decision(
        decision: &SupervisorTurnDecision,
        summary: Option<&ipc::SupervisorTurnDecisionSummary>,
        related: &[ipc::SupervisorTurnDecisionSummary],
    ) -> Vec<String> {
        let mut lines = vec![
            format!("decision_id: {}", decision.decision_id),
            format!("assignment_id: {}", decision.assignment_id),
            format!("thread_id: {}", decision.codex_thread_id),
            format!("kind: {:?}", decision.kind),
            format!("proposal_kind: {:?}", decision.proposal_kind),
            format!("status: {:?}", decision.status),
            format!(
                "actionable: {}",
                if decision.status == SupervisorTurnDecisionStatus::ProposedToHuman {
                    "yes"
                } else {
                    "no"
                }
            ),
            format!(
                "editable: {}",
                if decision.kind == SupervisorTurnDecisionKind::SteerActiveTurn
                    && decision.status == SupervisorTurnDecisionStatus::ProposedToHuman
                {
                    "yes"
                } else {
                    "no"
                }
            ),
            format!(
                "basis_turn_id: {}",
                decision.basis_turn_id.as_deref().unwrap_or("")
            ),
            format!("created_at: {}", decision.created_at),
            format!("rationale_summary: {}", decision.rationale_summary),
        ];

        if let Some(summary) = summary {
            lines.push(format!(
                "workstream_id: {}",
                summary.workstream_id.as_deref().unwrap_or("")
            ));
            lines.push(format!(
                "work_unit_id: {}",
                summary.work_unit_id.as_deref().unwrap_or("")
            ));
            lines.push(format!(
                "supervisor_id: {}",
                summary.supervisor_id.as_deref().unwrap_or("")
            ));
            lines.push(format!("open: {}", summary.open));
        }

        if let Some(approved_at) = decision.approved_at.as_ref() {
            lines.push(format!("approved_at: {approved_at}"));
        }
        if let Some(rejected_at) = decision.rejected_at.as_ref() {
            lines.push(format!("rejected_at: {rejected_at}"));
        }
        if let Some(sent_at) = decision.sent_at.as_ref() {
            lines.push(format!("sent_at: {sent_at}"));
        }
        if let Some(sent_turn_id) = decision.sent_turn_id.as_ref() {
            lines.push(format!("sent_turn_id: {sent_turn_id}"));
        }
        if let Some(superseded_by) = decision.superseded_by.as_ref() {
            lines.push(format!("superseded_by: {superseded_by}"));
        }
        if let Some(notes) = decision.notes.as_ref() {
            lines.push(format!("notes: {notes}"));
        }
        lines.extend(Self::format_text_block(
            "proposed_text",
            decision.proposed_text.as_deref(),
        ));

        let related_decisions = if related.is_empty() {
            Vec::new()
        } else {
            related
                .iter()
                .map(Self::format_supervisor_turn_decision_summary)
                .collect::<Vec<_>>()
        };
        if !related_decisions.is_empty() {
            lines.push("related_history:".to_string());
            lines.extend(
                related_decisions
                    .into_iter()
                    .map(|line| format!("  {line}")),
            );
        }
        let chains = Self::format_decision_revision_chains(related);
        if !chains.is_empty() {
            lines.push("revision_chains:".to_string());
            lines.extend(chains.into_iter().map(|line| format!("  {line}")));
        }

        lines
    }

    fn format_supervisor_turn_decision_summary(
        decision: &ipc::SupervisorTurnDecisionSummary,
    ) -> String {
        let text_preview = decision
            .proposed_text
            .as_deref()
            .map(Self::single_line_preview)
            .unwrap_or_else(|| "-".to_string());
        let rationale_preview = Self::single_line_preview(&decision.rationale_summary);
        let superseded_by = decision.superseded_by.as_deref().unwrap_or("-");
        format!(
            "{}\t{:?}\t{:?}/{:?}\tthread={}\tassignment={}\tws={}\twu={}\tsupervisor={}\tbasis={}\tcreated={}\tsuperseded_by={}\trationale={}\ttext={}",
            decision.decision_id,
            decision.status,
            decision.kind,
            decision.proposal_kind,
            decision.codex_thread_id,
            decision.assignment_id,
            decision.workstream_id.as_deref().unwrap_or("-"),
            decision.work_unit_id.as_deref().unwrap_or("-"),
            decision.supervisor_id.as_deref().unwrap_or("-"),
            decision.basis_turn_id.as_deref().unwrap_or("-"),
            decision.created_at,
            superseded_by,
            rationale_preview,
            text_preview
        )
    }

    fn format_decision_revision_chains(
        decisions: &[ipc::SupervisorTurnDecisionSummary],
    ) -> Vec<String> {
        use std::collections::{BTreeSet, HashMap};

        let by_id = decisions
            .iter()
            .map(|decision| (decision.decision_id.as_str(), decision))
            .collect::<HashMap<_, _>>();
        let child_ids = decisions
            .iter()
            .filter_map(|decision| decision.superseded_by.as_deref())
            .collect::<BTreeSet<_>>();

        let mut chains = Vec::new();
        for root in decisions.iter().filter(|decision| {
            decision.superseded_by.is_some() && !child_ids.contains(decision.decision_id.as_str())
        }) {
            let mut chain = vec![root.decision_id.clone()];
            let mut next_id = root.superseded_by.as_deref();
            while let Some(id) = next_id {
                chain.push(id.to_string());
                next_id = by_id
                    .get(id)
                    .and_then(|decision| decision.superseded_by.as_deref());
            }
            if chain.len() > 1 {
                chains.push(chain.join(" -> "));
            }
        }
        chains.sort();
        chains
    }

    fn format_text_block(label: &str, text: Option<&str>) -> Vec<String> {
        match text {
            Some(text) => {
                let mut lines = vec![format!("{label}:")];
                lines.extend(text.lines().map(|line| format!("  {line}")));
                if text.ends_with('\n') {
                    lines.push("  ".to_string());
                }
                lines
            }
            None => vec![format!("{label}:")],
        }
    }

    fn single_line_preview(text: &str) -> String {
        let preview = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if preview.is_empty() {
            "-".to_string()
        } else {
            preview
        }
    }

    async fn ready_client(&self) -> Result<Arc<OrcasIpcClient>> {
        let launch = if self.overrides.force_spawn {
            OrcasDaemonLaunch::Always
        } else {
            OrcasDaemonLaunch::IfNeeded
        };
        let mut last_error: Option<Error> = None;
        let mut delay = Duration::from_millis(100);

        for _ in 0..5 {
            let client = self.connect_client(launch).await?;
            match client.daemon_connect().await {
                Ok(_) => return Ok(client),
                Err(error) => {
                    last_error = Some(Error::new(error).context("connect Orcas daemon to Codex"));
                    sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_millis(800));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::msg("connect Orcas daemon to Codex")))
    }

    async fn daemon_state_client(&self) -> Result<Arc<OrcasIpcClient>> {
        let launch = if self.overrides.force_spawn {
            OrcasDaemonLaunch::Always
        } else {
            OrcasDaemonLaunch::IfNeeded
        };
        self.connect_client(launch).await
    }

    async fn connect_client(&self, launch: OrcasDaemonLaunch) -> Result<Arc<OrcasIpcClient>> {
        let mut last_error: Option<Error> = None;
        let mut delay = Duration::from_millis(100);

        for _ in 0..5 {
            match self.daemon.ensure_running(launch).await {
                Ok(_) => match OrcasIpcClient::connect(&self.paths).await {
                    Ok(client) => return Ok(client),
                    Err(error) => {
                        last_error = Some(Error::new(error).context("connect to Orcas daemon"));
                    }
                },
                Err(error) => {
                    last_error = Some(Error::new(error));
                }
            }
            sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(800));
        }

        Err(last_error.unwrap_or_else(|| Error::msg("connect to Orcas daemon")))
    }

    fn print_event_summary(event: &ipc::EventSummary) {
        println!(
            "{}\t{}\tthread={}\tturn={}\t{}",
            event.timestamp.to_rfc3339(),
            event.kind,
            event.thread_id.as_deref().unwrap_or("-"),
            event.turn_id.as_deref().unwrap_or("-"),
            event.message
        );
    }

    fn print_daemon_event(envelope: &ipc::DaemonEventEnvelope) {
        let (kind, thread_id, turn_id, message) = Self::daemon_event_line_parts(&envelope.event);
        println!(
            "{}\t{}\tthread={}\tturn={}\t{}",
            envelope.emitted_at.to_rfc3339(),
            kind,
            thread_id.unwrap_or("-"),
            turn_id.unwrap_or("-"),
            message
        );
    }

    fn daemon_event_line_parts(
        event: &ipc::DaemonEvent,
    ) -> (&'static str, Option<&str>, Option<&str>, String) {
        match event {
            ipc::DaemonEvent::UpstreamStatusChanged { upstream } => (
                "upstream",
                None,
                None,
                format!(
                    "endpoint={} status={} detail={}",
                    upstream.endpoint,
                    upstream.status,
                    upstream.detail.as_deref().unwrap_or("-")
                ),
            ),
            ipc::DaemonEvent::SessionChanged { session } => (
                "session",
                session.active_thread_id.as_deref(),
                None,
                format!("active_turns={}", session.active_turns.len()),
            ),
            ipc::DaemonEvent::ThreadUpdated { thread } => (
                "thread",
                Some(thread.id.as_str()),
                None,
                format!(
                    "status={} scope={} in_flight={} preview={}",
                    thread.status,
                    thread.scope,
                    thread.turn_in_flight,
                    Self::truncate_snippet(&thread.preview.replace('\n', " "))
                ),
            ),
            ipc::DaemonEvent::TurnUpdated { thread_id, turn } => (
                "turn",
                Some(thread_id.as_str()),
                Some(turn.id.as_str()),
                format!("status={} items={}", turn.status, turn.items.len()),
            ),
            ipc::DaemonEvent::ItemUpdated {
                thread_id,
                turn_id,
                item,
            } => (
                "item",
                Some(thread_id.as_str()),
                Some(turn_id.as_str()),
                format!(
                    "item_id={} type={} status={}",
                    item.id,
                    item.item_type,
                    item.status.as_deref().unwrap_or("-")
                ),
            ),
            ipc::DaemonEvent::OutputDelta {
                thread_id,
                turn_id,
                delta,
                ..
            } => (
                "delta",
                Some(thread_id.as_str()),
                Some(turn_id.as_str()),
                Self::truncate_snippet(&delta.replace('\n', "\\n")),
            ),
            ipc::DaemonEvent::TurnDiffUpdated {
                thread_id,
                turn_id,
                diff,
            } => (
                "turn_diff",
                Some(thread_id.as_str()),
                Some(turn_id.as_str()),
                Self::truncate_snippet(diff),
            ),
            ipc::DaemonEvent::TurnPlanUpdated {
                thread_id,
                turn_id,
                plan,
            } => (
                "turn_plan",
                Some(thread_id.as_str()),
                Some(turn_id.as_str()),
                format!("steps={}", plan.plan.len()),
            ),
            ipc::DaemonEvent::ThreadTokenUsageUpdated {
                thread_id,
                token_usage,
            } => (
                "thread_token_usage",
                Some(thread_id.as_str()),
                None,
                format!("total_tokens={}", token_usage.total_tokens),
            ),
            ipc::DaemonEvent::WorkstreamLifecycle { action, workstream } => (
                "workstream",
                None,
                None,
                format!("workstream_id={} action={action:?}", workstream.id),
            ),
            ipc::DaemonEvent::WorkUnitLifecycle { action, work_unit } => (
                "work_unit",
                None,
                None,
                format!("work_unit_id={} action={action:?}", work_unit.id),
            ),
            ipc::DaemonEvent::TrackedThreadLifecycle {
                action,
                tracked_thread,
            } => (
                "tracked_thread",
                None,
                None,
                format!("tracked_thread_id={} action={action:?}", tracked_thread.id),
            ),
            ipc::DaemonEvent::AssignmentLifecycle { action, assignment } => (
                "assignment",
                None,
                None,
                format!("assignment_id={} action={action:?}", assignment.id),
            ),
            ipc::DaemonEvent::CodexAssignmentLifecycle { action, assignment } => (
                "codex_assignment",
                Some(assignment.codex_thread_id.as_str()),
                assignment.latest_basis_turn_id.as_deref(),
                format!(
                    "assignment_id={} action={action:?}",
                    assignment.assignment_id
                ),
            ),
            ipc::DaemonEvent::SupervisorDecisionLifecycle { action, decision } => (
                "supervisor_decision",
                Some(decision.codex_thread_id.as_str()),
                decision.basis_turn_id.as_deref(),
                format!("decision_id={} action={action:?}", decision.decision_id),
            ),
            ipc::DaemonEvent::ReportRecorded { report } => (
                "report",
                None,
                None,
                format!(
                    "report_id={} parse_result={:?}",
                    report.id, report.parse_result
                ),
            ),
            ipc::DaemonEvent::DecisionApplied { decision } => (
                "decision",
                None,
                None,
                format!(
                    "decision_id={} type={:?}",
                    decision.id, decision.decision_type
                ),
            ),
            ipc::DaemonEvent::ProposalLifecycle {
                action, proposal, ..
            } => (
                "proposal",
                None,
                None,
                format!("proposal_id={} action={action:?}", proposal.id),
            ),
            ipc::DaemonEvent::Warning { message } => ("warning", None, None, message.clone()),
        }
    }

    fn turn_recent_text(turn: &ipc::TurnView) -> Option<String> {
        turn.items.iter().rev().find_map(|item| {
            item.summary
                .clone()
                .or_else(|| item.text.clone())
                .or_else(|| item.payload.as_ref().map(Self::truncate_json_value))
        })
    }

    fn truncate_json_value(value: &serde_json::Value) -> String {
        Self::truncate_snippet(&value.to_string())
    }

    fn truncate_snippet(text: &str) -> String {
        const LIMIT: usize = 96;
        let compact = text.replace('\n', " ");
        let mut chars = compact.chars();
        let truncated: String = chars.by_ref().take(LIMIT).collect();
        if chars.next().is_some() {
            format!("{truncated}...")
        } else {
            truncated
        }
    }

    fn print_wrapped_field(label: &str, value: &str, width: usize) {
        let prefix = format!("{label}: ");
        let continuation = " ".repeat(prefix.len());
        let available = width.saturating_sub(prefix.len()).max(16);
        let mut lines = Vec::new();
        let mut current = String::new();

        for word in value.split_whitespace() {
            let candidate_len = if current.is_empty() {
                word.len()
            } else {
                current.len() + 1 + word.len()
            };
            if !current.is_empty() && candidate_len > available {
                lines.push(current);
                current = word.to_string();
            } else {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(word);
            }
        }

        if !current.is_empty() {
            lines.push(current);
        }

        if lines.is_empty() {
            println!("{prefix}");
            return;
        }

        for (index, line) in lines.iter().enumerate() {
            if index == 0 {
                println!("{prefix}{line}");
            } else {
                println!("{continuation}{line}");
            }
        }
    }

    fn print_discovered_app_server(server: &LocalCodexAppServer) {
        println!("endpoint: {}", server.endpoint);
        println!("pid: {}", server.pid);
        println!(
            "ppid: {}",
            server
                .parent_pid
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
        println!("process: {}", server.process_name);
        println!("listen: {}", server.listen_address);
        println!(
            "managed: {}",
            if server.managed {
                ORCAS_APP_SERVER_TAG_VALUE
            } else {
                "no"
            }
        );
        if let Some(owner_kind) = server.owner_kind.as_deref() {
            println!("owner_kind: {owner_kind}");
        }
        if let Some(owner_pid) = server.owner_pid {
            let running = server.owner_pid_running.unwrap_or(false);
            println!("owner_pid: {owner_pid} running={running}");
        }
        if let Some(owner_listen_url) = server.owner_listen_url.as_deref() {
            println!("owner_listen_url: {owner_listen_url}");
        }
        if let Some(owner_started_at) = server.owner_started_at.as_deref() {
            println!("owner_started_at: {owner_started_at}");
        }
        println!("reap_hint: {}", server.reap_hint);
        Self::print_wrapped_field("args", &server.command_line, 88);
    }

    async fn discover_local_codex_app_servers() -> Result<Vec<LocalCodexAppServer>> {
        let output = Command::new("ss")
            .args(["-ltnpH"])
            .output()
            .await
            .map_err(|error| anyhow!("failed to run `ss -ltnpH`: {error}"))?;
        if !output.status.success() {
            return Err(anyhow!("`ss -ltnpH` failed with status {}", output.status));
        }

        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| anyhow!("failed to decode `ss` output as utf-8: {error}"))?;
        let mut servers = Vec::new();
        for line in stdout.lines() {
            if let Some(server) = Self::parse_ss_codex_listener(line).await? {
                servers.push(server);
            }
        }
        servers.sort_by(|left, right| {
            left.endpoint
                .cmp(&right.endpoint)
                .then_with(|| left.pid.cmp(&right.pid))
        });
        Ok(servers)
    }

    async fn parse_ss_codex_listener(line: &str) -> Result<Option<LocalCodexAppServer>> {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.contains("users:(") {
            return Ok(None);
        }

        let columns = trimmed.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 6 {
            return Ok(None);
        }
        let listen_address = columns[3].to_string();
        let Some(users_index) = trimmed.find("users:(") else {
            return Ok(None);
        };
        let users = &trimmed[users_index..];
        let Some(process_name) = Self::extract_quoted_value(users) else {
            return Ok(None);
        };
        if process_name != "codex" {
            return Ok(None);
        }
        let Some(pid) = Self::extract_pid(users) else {
            return Ok(None);
        };

        let command_line = Self::read_process_cmdline(pid)
            .await
            .unwrap_or_else(|| process_name.to_string());
        if !(command_line.contains("app-server") || command_line.contains("--listen")) {
            return Ok(None);
        }

        let environment = Self::read_process_environment(pid)
            .await
            .unwrap_or_default();
        let parent_pid = Self::read_process_status_number(pid, "PPid:").await;
        let owner_pid = environment
            .get(ORCAS_APP_SERVER_OWNER_PID_ENV)
            .and_then(|value| value.parse().ok());
        let owner_pid_running = owner_pid.map(Self::process_exists);
        let managed = environment
            .get(ORCAS_APP_SERVER_TAG_ENV)
            .is_some_and(|value| value == ORCAS_APP_SERVER_TAG_VALUE);
        let owner_kind = environment.get(ORCAS_APP_SERVER_OWNER_KIND_ENV).cloned();
        let owner_listen_url = environment.get(ORCAS_APP_SERVER_LISTEN_URL_ENV).cloned();
        let owner_started_at = environment.get(ORCAS_APP_SERVER_STARTED_AT_ENV).cloned();
        let orphaned = parent_pid == Some(1);
        let reap_hint = if managed {
            if owner_pid_running == Some(false) || owner_pid.is_none() {
                "safe-tagged-owner-dead"
            } else {
                "tagged-owner-alive"
            }
        } else if orphaned {
            "untagged-orphan-manual-review"
        } else {
            "untagged-manual-review"
        };

        Ok(Some(LocalCodexAppServer {
            endpoint: format!("ws://{listen_address}"),
            listen_address,
            pid,
            parent_pid,
            process_name: process_name.to_string(),
            command_line,
            managed,
            owner_kind,
            owner_pid,
            owner_pid_running,
            owner_listen_url,
            owner_started_at,
            reap_hint: reap_hint.to_string(),
        }))
    }

    fn extract_quoted_value(text: &str) -> Option<&str> {
        let start = text.find('"')?;
        let rest = &text[start + 1..];
        let end = rest.find('"')?;
        Some(&rest[..end])
    }

    fn extract_pid(text: &str) -> Option<u32> {
        let pid_marker = "pid=";
        let start = text.find(pid_marker)? + pid_marker.len();
        let digits = text[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect::<String>();
        digits.parse().ok()
    }

    async fn read_process_cmdline(pid: u32) -> Option<String> {
        let path = format!("/proc/{pid}/cmdline");
        let bytes = tokio::fs::read(path).await.ok()?;
        if bytes.is_empty() {
            return None;
        }
        let parts = bytes
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }

    async fn read_process_environment(pid: u32) -> Option<BTreeMap<String, String>> {
        let path = format!("/proc/{pid}/environ");
        let bytes = tokio::fs::read(path).await.ok()?;
        if bytes.is_empty() {
            return Some(BTreeMap::new());
        }

        let mut env = BTreeMap::new();
        for entry in bytes
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            let text = String::from_utf8_lossy(entry);
            let Some((key, value)) = text.split_once('=') else {
                continue;
            };
            env.insert(key.to_string(), value.to_string());
        }
        Some(env)
    }

    async fn read_process_status_number(pid: u32, key: &str) -> Option<u32> {
        let path = format!("/proc/{pid}/status");
        let contents = tokio::fs::read_to_string(path).await.ok()?;
        contents.lines().find_map(|line| {
            let value = line.strip_prefix(key)?.trim();
            value.parse().ok()
        })
    }

    fn process_exists(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    fn print_proposal_record(proposal: &SupervisorProposalRecord) {
        println!("proposal_id: {}", proposal.id);
        println!("workstream_id: {}", proposal.workstream_id);
        println!("work_unit_id: {}", proposal.primary_work_unit_id);
        println!("source_report_id: {}", proposal.source_report_id);
        println!("status: {:?}", proposal.status);
        println!("created_at: {}", proposal.created_at);
        println!("trigger_kind: {:?}", proposal.trigger.kind);
        println!("trigger_requested_by: {}", proposal.trigger.requested_by);
        println!("reasoner_backend: {}", proposal.reasoner_backend);
        println!("reasoner_model: {}", proposal.reasoner_model);
        if let Some(response_id) = proposal.reasoner_response_id.as_ref() {
            println!("reasoner_response_id: {response_id}");
        }
        if let Some(validated_at) = proposal.validated_at.as_ref() {
            println!("validated_at: {validated_at}");
        }
        if let Some(output_text) = proposal.reasoner_output_text.as_ref() {
            println!("reasoner_output_text: {output_text}");
        }
        if let Some(model_proposal) = proposal.proposal.as_ref() {
            println!(
                "model_proposal_schema_version: {}",
                model_proposal.schema_version
            );
            println!(
                "model_summary_headline: {}",
                model_proposal.summary.headline
            );
            println!(
                "model_summary_situation: {}",
                model_proposal.summary.situation
            );
            println!(
                "model_summary_recommended_action: {}",
                model_proposal.summary.recommended_action
            );
            println!(
                "model_proposed_decision_type: {:?}",
                model_proposal.proposed_decision.decision_type
            );
            println!(
                "model_proposed_decision_rationale: {}",
                model_proposal.proposed_decision.rationale
            );
            println!(
                "model_expected_work_unit_status: {}",
                model_proposal.proposed_decision.expected_work_unit_status
            );
            println!(
                "model_requires_assignment: {}",
                model_proposal.proposed_decision.requires_assignment
            );
            println!("model_confidence: {:?}", model_proposal.confidence);
            if !model_proposal.summary.key_evidence.is_empty() {
                println!(
                    "model_key_evidence: {}",
                    model_proposal.summary.key_evidence.join(" | ")
                );
            }
            if !model_proposal.summary.risks.is_empty() {
                println!("model_risks: {}", model_proposal.summary.risks.join(" | "));
            }
            if !model_proposal.summary.review_focus.is_empty() {
                println!(
                    "model_review_focus: {}",
                    model_proposal.summary.review_focus.join(" | ")
                );
            }
            if !model_proposal.warnings.is_empty() {
                println!("model_warnings: {}", model_proposal.warnings.join(" | "));
            }
            if !model_proposal.open_questions.is_empty() {
                println!(
                    "model_open_questions: {}",
                    model_proposal.open_questions.join(" | ")
                );
            }
            if let Some(draft) = model_proposal.draft_next_assignment.as_ref() {
                Self::print_draft_assignment("model", draft);
            }
        } else {
            println!("model_proposal: none");
        }
        if let Some(failure) = proposal.generation_failure.as_ref() {
            println!("generation_failure_stage: {:?}", failure.stage);
            println!("generation_failure_message: {}", failure.message);
        }
        if let Some(edits) = proposal.approval_edits.as_ref() {
            println!("approval_edits_present: true");
            if edits.is_empty() {
                println!("approval_edits: none");
            } else {
                if let Some(decision_type) = edits.decision_type {
                    println!("approval_edit_decision_type: {:?}", decision_type);
                }
                if let Some(rationale) = edits.decision_rationale.as_ref() {
                    println!("approval_edit_decision_rationale: {rationale}");
                }
                if let Some(worker_id) = edits.preferred_worker_id.as_ref() {
                    println!("approval_edit_preferred_worker_id: {worker_id}");
                }
                if let Some(worker_kind) = edits.worker_kind.as_ref() {
                    println!("approval_edit_worker_kind: {worker_kind}");
                }
                if let Some(objective) = edits.objective.as_ref() {
                    println!("approval_edit_objective: {objective}");
                }
                if !edits.instructions.is_empty() {
                    println!(
                        "approval_edit_instructions: {}",
                        edits.instructions.join(" | ")
                    );
                }
                if !edits.acceptance_criteria.is_empty() {
                    println!(
                        "approval_edit_acceptance_criteria: {}",
                        edits.acceptance_criteria.join(" | ")
                    );
                }
                if !edits.stop_conditions.is_empty() {
                    println!(
                        "approval_edit_stop_conditions: {}",
                        edits.stop_conditions.join(" | ")
                    );
                }
                if !edits.expected_report_fields.is_empty() {
                    println!(
                        "approval_edit_expected_report_fields: {}",
                        edits.expected_report_fields.join(",")
                    );
                }
            }
        }
        if let Some(approved_proposal) = proposal.approved_proposal.as_ref() {
            println!(
                "approved_proposed_decision_type: {:?}",
                approved_proposal.proposed_decision.decision_type
            );
            println!(
                "approved_proposed_decision_rationale: {}",
                approved_proposal.proposed_decision.rationale
            );
            if let Some(draft) = approved_proposal.draft_next_assignment.as_ref() {
                Self::print_draft_assignment("approved", draft);
            }
        }
        if let Some(reviewed_at) = proposal.reviewed_at.as_ref() {
            println!("reviewed_at: {reviewed_at}");
        }
        if let Some(reviewed_by) = proposal.reviewed_by.as_ref() {
            println!("reviewed_by: {reviewed_by}");
        }
        if let Some(review_note) = proposal.review_note.as_ref() {
            println!("review_note: {review_note}");
        }
        if let Some(decision_id) = proposal.approved_decision_id.as_ref() {
            println!("approved_decision_id: {decision_id}");
        }
        if let Some(assignment_id) = proposal.approved_assignment_id.as_ref() {
            println!("approved_assignment_id: {assignment_id}");
        }
    }

    fn print_proposal_artifact_summary(summary: &ipc::SupervisorProposalArtifactSummary) {
        println!("proposal_id: {}", summary.proposal_id);
        println!("proposal_status: {:?}", summary.proposal_status);
        println!(
            "prompt_artifact_present: {}",
            summary.prompt_artifact_present
        );
        if let Some(version) = summary.prompt_template_version.as_ref() {
            println!("prompt_template_version: {version}");
        }
        if let Some(hash) = summary.prompt_hash.as_ref() {
            println!("prompt_hash: {hash}");
        }
        if let Some(hash) = summary.request_body_hash.as_ref() {
            println!("request_body_hash: {hash}");
        }
        println!(
            "response_artifact_present: {}",
            summary.response_artifact_present
        );
        if let Some(hash) = summary.response_hash.as_ref() {
            println!("response_hash: {hash}");
        }
        println!(
            "raw_response_body_present: {}",
            summary.raw_response_body_present
        );
        if let Some(hash) = summary.raw_response_body_hash.as_ref() {
            println!("raw_response_body_hash: {hash}");
        }
        println!("reasoner_backend: {}", summary.reasoner_backend);
        println!("reasoner_model: {}", summary.reasoner_model);
        if let Some(response_id) = summary.reasoner_response_id.as_ref() {
            println!("reasoner_response_id: {response_id}");
        }
        println!(
            "parsed_proposal_present: {}",
            summary.parsed_proposal_present
        );
        println!(
            "approved_proposal_present: {}",
            summary.approved_proposal_present
        );
        if let Some(stage) = summary.generation_failure_stage {
            println!("generation_failure_stage: {:?}", stage);
        }
    }

    fn print_proposal_artifact_detail(detail: &ipc::SupervisorProposalArtifactDetail) {
        println!("proposal_id: {}", detail.proposal_id);
        println!("proposal_status: {:?}", detail.proposal_status);
        println!("created_at: {}", detail.created_at);
        if let Some(validated_at) = detail.validated_at.as_ref() {
            println!("validated_at: {validated_at}");
        }
        if let Some(reviewed_at) = detail.reviewed_at.as_ref() {
            println!("reviewed_at: {reviewed_at}");
        }
        println!("reasoner_backend: {}", detail.reasoner_backend);
        println!("reasoner_model: {}", detail.reasoner_model);
        if let Some(response_id) = detail.reasoner_response_id.as_ref() {
            println!("reasoner_response_id: {response_id}");
        }
        if let Some(prompt_render) = detail.prompt_render.as_ref() {
            println!(
                "prompt_template_version: {}",
                prompt_render.render_spec.template_version
            );
            println!(
                "prompt_context_schema_version: {}",
                prompt_render.render_spec.context_schema_version
            );
            println!(
                "prompt_proposal_schema_version: {}",
                prompt_render.render_spec.proposal_schema_version
            );
            println!("prompt_hash: {}", prompt_render.prompt_hash);
            if let Some(hash) = prompt_render.request_body_hash.as_ref() {
                println!("request_body_hash: {hash}");
            }
            println!("prompt_rendered_at: {}", prompt_render.rendered_at);
            println!(
                "prompt_instructions_text: {}",
                prompt_render.instructions_text
            );
            println!(
                "prompt_user_content_text: {}",
                prompt_render.user_content_text
            );
            println!(
                "prompt_context_pack_text: {}",
                prompt_render.context_pack_text
            );
        } else {
            println!("prompt_render: none");
        }
        if let Some(response_artifact) = detail.response_artifact.as_ref() {
            println!("response_hash: {}", response_artifact.response_hash);
            println!("response_backend_kind: {}", response_artifact.backend_kind);
            println!("response_model: {}", response_artifact.model);
            if let Some(response_id) = response_artifact.response_id.as_ref() {
                println!("response_artifact_id: {response_id}");
            }
            if let Some(usage) = response_artifact.usage.as_ref() {
                println!("response_usage_input_tokens: {:?}", usage.input_tokens);
                println!("response_usage_output_tokens: {:?}", usage.output_tokens);
                println!("response_usage_total_tokens: {:?}", usage.total_tokens);
            }
            println!("response_captured_at: {}", response_artifact.captured_at);
            if !response_artifact.output_items.is_empty() {
                println!(
                    "response_output_items: {}",
                    serde_json::to_string(&response_artifact.output_items)
                        .expect("serialize response output items")
                );
            }
            if let Some(output_text) = response_artifact.extracted_output_text.as_ref() {
                println!("response_extracted_output_text: {output_text}");
            }
            if let Some(raw_body_hash) = response_artifact.raw_response_body_hash.as_ref() {
                println!("raw_response_body_hash: {raw_body_hash}");
            }
            if let Some(raw_body) = response_artifact.raw_response_body.as_ref() {
                println!("raw_response_body: {raw_body}");
            }
        } else {
            println!("response_artifact: none");
        }
        if let Some(output_text) = detail.reasoner_output_text.as_ref() {
            println!("reasoner_output_text: {output_text}");
        }
        if let Some(parsed_proposal) = detail.parsed_proposal.as_ref() {
            println!(
                "parsed_proposal: {}",
                serde_json::to_string(parsed_proposal).expect("serialize parsed proposal")
            );
        } else {
            println!("parsed_proposal: none");
        }
        if let Some(approved_proposal) = detail.approved_proposal.as_ref() {
            println!(
                "approved_proposal: {}",
                serde_json::to_string(approved_proposal).expect("serialize approved proposal")
            );
        }
        if let Some(failure) = detail.generation_failure.as_ref() {
            println!("generation_failure_stage: {:?}", failure.stage);
            println!("generation_failure_message: {}", failure.message);
        }
    }

    fn render_proposal_artifact_export_json(
        export: &ipc::SupervisorProposalArtifactExport,
    ) -> Result<String> {
        Ok(format!("{}\n", serde_json::to_string_pretty(export)?))
    }

    fn render_proposal_artifact_export_markdown(
        export: &ipc::SupervisorProposalArtifactExport,
    ) -> Result<String> {
        let mut out = String::new();
        out.push_str("# Supervisor Proposal Artifact Export\n\n");
        out.push_str("## Proposal Metadata\n");
        out.push_str(&format!("- Proposal ID: `{}`\n", export.proposal_id));
        out.push_str(&format!(
            "- Work Unit ID: `{}`\n",
            export.primary_work_unit_id
        ));
        out.push_str(&format!(
            "- Source Report ID: `{}`\n",
            export.source_report_id
        ));
        out.push_str(&format!("- Status: `{:?}`\n", export.proposal_status));
        out.push_str(&format!("- Created At: `{}`\n", export.created_at));
        out.push_str(&format!(
            "- Validated At: `{}`\n",
            export
                .validated_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        ));
        out.push_str(&format!(
            "- Reviewed At: `{}`\n",
            export
                .reviewed_at
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string())
        ));
        out.push_str(&format!(
            "- Reviewed By: `{}`\n",
            export.reviewed_by.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Review Note: `{}`\n",
            export.review_note.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Approved Decision ID: `{}`\n",
            export.approved_decision_id.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Approved Assignment ID: `{}`\n\n",
            export.approved_assignment_id.as_deref().unwrap_or("-")
        ));

        out.push_str("## Artifact Summary\n");
        out.push_str(&format!(
            "- Prompt Artifact Present: `{}`\n",
            export.artifact_summary.prompt_artifact_present
        ));
        out.push_str(&format!(
            "- Prompt Template Version: `{}`\n",
            export
                .artifact_summary
                .prompt_template_version
                .as_deref()
                .unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Prompt Hash: `{}`\n",
            export
                .artifact_summary
                .prompt_hash
                .as_deref()
                .unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Request Body Hash: `{}`\n",
            export
                .artifact_summary
                .request_body_hash
                .as_deref()
                .unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Response Artifact Present: `{}`\n",
            export.artifact_summary.response_artifact_present
        ));
        out.push_str(&format!(
            "- Response Hash: `{}`\n",
            export
                .artifact_summary
                .response_hash
                .as_deref()
                .unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Raw Response Body Present: `{}`\n",
            export.artifact_summary.raw_response_body_present
        ));
        out.push_str(&format!(
            "- Raw Response Body Hash: `{}`\n",
            export
                .artifact_summary
                .raw_response_body_hash
                .as_deref()
                .unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Reasoner Backend: `{}`\n",
            export.artifact_summary.reasoner_backend
        ));
        out.push_str(&format!(
            "- Reasoner Model: `{}`\n",
            export.artifact_summary.reasoner_model
        ));
        out.push_str(&format!(
            "- Reasoner Response ID: `{}`\n",
            export
                .artifact_summary
                .reasoner_response_id
                .as_deref()
                .unwrap_or("-")
        ));
        out.push_str(&format!(
            "- Parsed Proposal Present: `{}`\n",
            export.artifact_summary.parsed_proposal_present
        ));
        out.push_str(&format!(
            "- Approved Proposal Present: `{}`\n",
            export.artifact_summary.approved_proposal_present
        ));
        out.push_str(&format!(
            "- Generation Failure Stage: `{}`\n\n",
            export
                .artifact_summary
                .generation_failure_stage
                .map(|value| format!("{value:?}"))
                .unwrap_or_else(|| "-".to_string())
        ));

        Self::push_markdown_json_section(
            &mut out,
            "Prompt Artifact",
            serde_json::to_value(&export.artifact_detail.prompt_render)?,
        )?;
        Self::push_markdown_json_section(
            &mut out,
            "Response Artifact",
            serde_json::to_value(&export.artifact_detail.response_artifact)?,
        )?;
        Self::push_markdown_text_section(
            &mut out,
            "Extracted Output Text",
            export.artifact_detail.reasoner_output_text.as_deref(),
        );
        Self::push_markdown_json_section(
            &mut out,
            "Parsed Proposal",
            serde_json::to_value(&export.artifact_detail.parsed_proposal)?,
        )?;
        Self::push_markdown_json_section(
            &mut out,
            "Approved Proposal",
            serde_json::to_value(&export.artifact_detail.approved_proposal)?,
        )?;
        Self::push_markdown_json_section(
            &mut out,
            "Failure Metadata",
            serde_json::to_value(&export.artifact_detail.generation_failure)?,
        )?;
        Ok(out)
    }

    fn push_markdown_json_section(
        out: &mut String,
        title: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        out.push_str(&format!("## {title}\n"));
        out.push_str("```json\n");
        out.push_str(&serde_json::to_string_pretty(&value)?);
        out.push_str("\n```\n\n");
        Ok(())
    }

    fn push_markdown_text_section(out: &mut String, title: &str, value: Option<&str>) {
        out.push_str(&format!("## {title}\n"));
        match value {
            Some(value) => {
                out.push_str("```text\n");
                out.push_str(value);
                if !value.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("```\n\n");
            }
            None => out.push_str("_none_\n\n"),
        }
    }

    fn print_draft_assignment(prefix: &str, draft: &orcas_core::DraftAssignment) {
        println!(
            "{prefix}_draft_assignment_target_work_unit_id: {}",
            draft.target_work_unit_id
        );
        println!(
            "{prefix}_draft_assignment_predecessor_assignment_id: {}",
            draft.predecessor_assignment_id
        );
        println!(
            "{prefix}_draft_assignment_derived_from_decision_type: {:?}",
            draft.derived_from_decision_type
        );
        if let Some(worker_id) = draft.preferred_worker_id.as_ref() {
            println!("{prefix}_draft_assignment_preferred_worker_id: {worker_id}");
        }
        if let Some(worker_kind) = draft.worker_kind.as_ref() {
            println!("{prefix}_draft_assignment_worker_kind: {worker_kind}");
        }
        println!("{prefix}_draft_assignment_objective: {}", draft.objective);
        if !draft.instructions.is_empty() {
            println!(
                "{prefix}_draft_assignment_instructions: {}",
                draft.instructions.join(" | ")
            );
        }
        if !draft.acceptance_criteria.is_empty() {
            println!(
                "{prefix}_draft_assignment_acceptance_criteria: {}",
                draft.acceptance_criteria.join(" | ")
            );
        }
        if !draft.stop_conditions.is_empty() {
            println!(
                "{prefix}_draft_assignment_stop_conditions: {}",
                draft.stop_conditions.join(" | ")
            );
        }
        if !draft.required_context_refs.is_empty() {
            println!(
                "{prefix}_draft_assignment_required_context_refs: {}",
                draft.required_context_refs.join(",")
            );
        }
        if !draft.expected_report_fields.is_empty() {
            println!(
                "{prefix}_draft_assignment_expected_report_fields: {}",
                draft.expected_report_fields.join(",")
            );
        }
        println!(
            "{prefix}_draft_assignment_boundedness_note: {}",
            draft.boundedness_note
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use chrono::Utc;
    use orcas_core::{
        CodexThreadBootstrapState, CodexThreadSendPolicy, SupervisorPromptRenderArtifact,
        SupervisorPromptRenderSpec, SupervisorProposalFailure, SupervisorProposalFailureStage,
        SupervisorProposalStatus, SupervisorReasonerUsage, SupervisorResponseArtifact,
        SupervisorResponseContentPart, SupervisorResponseOutputItem, SupervisorTurnProposalKind,
    };

    #[derive(Debug, Default)]
    struct FakeSupervisorCodexBackend {
        state: Mutex<FakeSupervisorCodexState>,
        propose_error: Mutex<Option<String>>,
        approve_error: Mutex<Option<String>>,
    }

    #[derive(Debug, Default)]
    struct FakeSupervisorCodexState {
        next_decision_id: usize,
        assignments: Vec<ipc::CodexThreadAssignmentSummary>,
        decisions: BTreeMap<String, SupervisorTurnDecision>,
        last_approved_text: Option<String>,
    }

    #[async_trait]
    impl SupervisorCodexBackend for FakeSupervisorCodexBackend {
        async fn codex_assignment_list(
            &self,
            params: &ipc::CodexAssignmentListRequest,
        ) -> Result<ipc::CodexAssignmentListResponse> {
            let state = self.state.lock().expect("state");
            let assignments = state
                .assignments
                .iter()
                .filter(|assignment| {
                    params
                        .codex_thread_id
                        .as_ref()
                        .is_none_or(|thread_id| &assignment.codex_thread_id == thread_id)
                        && (params.include_inactive || assignment.active)
                })
                .cloned()
                .collect();
            Ok(ipc::CodexAssignmentListResponse { assignments })
        }

        async fn supervisor_decision_list(
            &self,
            params: &ipc::SupervisorDecisionListRequest,
        ) -> Result<ipc::SupervisorDecisionListResponse> {
            let state = self.state.lock().expect("state");
            let mut decisions = state
                .decisions
                .values()
                .filter(|decision| {
                    let assignment = state
                        .assignments
                        .iter()
                        .find(|assignment| assignment.assignment_id == decision.assignment_id);
                    params
                        .assignment_id
                        .as_ref()
                        .is_none_or(|assignment_id| &decision.assignment_id == assignment_id)
                        && params
                            .codex_thread_id
                            .as_ref()
                            .is_none_or(|thread_id| &decision.codex_thread_id == thread_id)
                        && params.workstream_id.as_ref().is_none_or(|workstream_id| {
                            assignment
                                .map(|assignment| &assignment.workstream_id == workstream_id)
                                .unwrap_or(false)
                        })
                        && params.work_unit_id.as_ref().is_none_or(|work_unit_id| {
                            assignment
                                .map(|assignment| &assignment.work_unit_id == work_unit_id)
                                .unwrap_or(false)
                        })
                        && params.supervisor_id.as_ref().is_none_or(|supervisor_id| {
                            assignment
                                .map(|assignment| &assignment.supervisor_id == supervisor_id)
                                .unwrap_or(false)
                        })
                        && params.status.is_none_or(|status| decision.status == status)
                        && params.kind.is_none_or(|kind| decision.kind == kind)
                        && (!params.actionable_only
                            || decision.status == SupervisorTurnDecisionStatus::ProposedToHuman)
                        && (params.include_closed
                            || params.status.is_some()
                            || params.actionable_only
                            || decision.status == SupervisorTurnDecisionStatus::ProposedToHuman)
                        && (params.include_superseded
                            || params.status == Some(SupervisorTurnDecisionStatus::Superseded)
                            || decision.status != SupervisorTurnDecisionStatus::Superseded)
                })
                .map(|decision| {
                    sample_decision_summary(
                        decision,
                        assignment_summary_for_decision(&state.assignments, decision),
                    )
                })
                .collect::<Vec<_>>();
            decisions.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| left.decision_id.cmp(&right.decision_id))
            });
            if let Some(limit) = params.limit {
                decisions.truncate(limit);
            }
            Ok(ipc::SupervisorDecisionListResponse { decisions })
        }

        async fn supervisor_decision_get(
            &self,
            params: &ipc::SupervisorDecisionGetRequest,
        ) -> Result<ipc::SupervisorDecisionGetResponse> {
            let state = self.state.lock().expect("state");
            let decision = state
                .decisions
                .get(&params.decision_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown supervisor decision `{}`", params.decision_id))?;
            Ok(ipc::SupervisorDecisionGetResponse { decision })
        }

        async fn supervisor_decision_propose_steer(
            &self,
            params: &ipc::SupervisorDecisionProposeSteerRequest,
        ) -> Result<ipc::SupervisorDecisionProposeSteerResponse> {
            if let Some(error) = self.propose_error.lock().expect("propose error").clone() {
                bail!("{error}");
            }

            let proposed_text = params
                .proposed_text
                .as_deref()
                .ok_or_else(|| anyhow!("steer proposal requires non-empty proposed_text"))?;
            let proposed_text =
                SupervisorService::require_non_empty_text("steer text", proposed_text)?;

            let mut state = self.state.lock().expect("state");
            let assignment = state
                .assignments
                .iter()
                .find(|assignment| assignment.assignment_id == params.assignment_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown assignment `{}`", params.assignment_id))?;
            let decision_id = format!("std-{}", state.next_decision_id);
            state.next_decision_id += 1;
            let decision = sample_decision(
                &decision_id,
                &assignment.assignment_id,
                &assignment.codex_thread_id,
                Some("turn-active-1"),
                SupervisorTurnDecisionKind::SteerActiveTurn,
                SupervisorTurnDecisionStatus::ProposedToHuman,
                Some(&proposed_text),
            );
            state
                .decisions
                .insert(decision.decision_id.clone(), decision.clone());
            Ok(ipc::SupervisorDecisionProposeSteerResponse { decision })
        }

        async fn supervisor_decision_replace_pending_steer(
            &self,
            params: &ipc::SupervisorDecisionReplacePendingSteerRequest,
        ) -> Result<ipc::SupervisorDecisionReplacePendingSteerResponse> {
            let proposed_text =
                SupervisorService::require_non_empty_text("steer text", &params.proposed_text)?;
            let mut state = self.state.lock().expect("state");
            let existing = state
                .decisions
                .get(&params.decision_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown supervisor decision `{}`", params.decision_id))?;
            if existing.kind != SupervisorTurnDecisionKind::SteerActiveTurn {
                bail!(
                    "supervisor decision `{}` is not a steer decision",
                    params.decision_id
                );
            }
            if existing.status != SupervisorTurnDecisionStatus::ProposedToHuman {
                bail!(
                    "supervisor decision `{}` is no longer editable",
                    params.decision_id
                );
            }

            let replacement_id = format!("std-{}", state.next_decision_id);
            state.next_decision_id += 1;
            let existing_decision = state
                .decisions
                .get_mut(&params.decision_id)
                .expect("existing decision");
            existing_decision.status = SupervisorTurnDecisionStatus::Superseded;
            existing_decision.superseded_by = Some(replacement_id.clone());

            let replacement = sample_decision(
                &replacement_id,
                &existing.assignment_id,
                &existing.codex_thread_id,
                existing.basis_turn_id.as_deref(),
                SupervisorTurnDecisionKind::SteerActiveTurn,
                SupervisorTurnDecisionStatus::ProposedToHuman,
                Some(&proposed_text),
            );
            state
                .decisions
                .insert(replacement.decision_id.clone(), replacement.clone());
            Ok(ipc::SupervisorDecisionReplacePendingSteerResponse {
                decision: replacement,
            })
        }

        async fn supervisor_decision_record_no_action(
            &self,
            params: &ipc::SupervisorDecisionRecordNoActionRequest,
        ) -> Result<ipc::SupervisorDecisionRecordNoActionResponse> {
            let mut state = self.state.lock().expect("state");
            let existing = state
                .decisions
                .get(&params.decision_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown supervisor decision `{}`", params.decision_id))?;
            if existing.kind != SupervisorTurnDecisionKind::NextTurn {
                bail!(
                    "supervisor decision `{}` is not a next-turn decision",
                    params.decision_id
                );
            }
            if existing.status != SupervisorTurnDecisionStatus::ProposedToHuman {
                bail!(
                    "supervisor decision `{}` is not pending human review",
                    params.decision_id
                );
            }

            let recorded_id = format!("std-{}", state.next_decision_id);
            state.next_decision_id += 1;
            let previous = state
                .decisions
                .get_mut(&params.decision_id)
                .expect("existing decision");
            previous.status = SupervisorTurnDecisionStatus::Superseded;
            previous.superseded_by = Some(recorded_id.clone());

            let recorded = sample_decision(
                &recorded_id,
                &existing.assignment_id,
                &existing.codex_thread_id,
                existing.basis_turn_id.as_deref(),
                SupervisorTurnDecisionKind::NoAction,
                SupervisorTurnDecisionStatus::Recorded,
                None,
            );
            state
                .decisions
                .insert(recorded.decision_id.clone(), recorded.clone());
            Ok(ipc::SupervisorDecisionRecordNoActionResponse { decision: recorded })
        }

        async fn supervisor_decision_manual_refresh(
            &self,
            params: &ipc::SupervisorDecisionManualRefreshRequest,
        ) -> Result<ipc::SupervisorDecisionManualRefreshResponse> {
            let mut state = self.state.lock().expect("state");
            let assignment = state
                .assignments
                .iter()
                .find(|assignment| assignment.assignment_id == params.assignment_id)
                .cloned()
                .ok_or_else(|| anyhow!("unknown assignment `{}`", params.assignment_id))?;
            if assignment.status != CodexThreadAssignmentStatus::Active {
                bail!(
                    "Codex thread assignment `{}` is not active",
                    assignment.assignment_id
                );
            }
            if state.decisions.values().any(|decision| {
                decision.assignment_id == assignment.assignment_id
                    && decision.status == SupervisorTurnDecisionStatus::ProposedToHuman
            }) {
                bail!(
                    "assignment `{}` already has open supervisor decision",
                    assignment.assignment_id
                );
            }
            let latest = state
                .decisions
                .values()
                .filter(|decision| decision.assignment_id == assignment.assignment_id)
                .max_by(|left, right| {
                    left.created_at
                        .cmp(&right.created_at)
                        .then_with(|| left.decision_id.cmp(&right.decision_id))
                })
                .cloned()
                .ok_or_else(|| {
                    anyhow!(
                        "assignment `{}` has no recorded no_action for the current basis",
                        assignment.assignment_id
                    )
                })?;
            if latest.kind != SupervisorTurnDecisionKind::NoAction
                || latest.status != SupervisorTurnDecisionStatus::Recorded
            {
                bail!(
                    "assignment `{}` has no recorded no_action for the current basis",
                    assignment.assignment_id
                );
            }

            let decision_id = format!("std-{}", state.next_decision_id);
            state.next_decision_id += 1;
            let decision = SupervisorTurnDecision {
                decision_id,
                assignment_id: assignment.assignment_id.clone(),
                codex_thread_id: assignment.codex_thread_id.clone(),
                basis_turn_id: latest.basis_turn_id.clone(),
                kind: SupervisorTurnDecisionKind::NextTurn,
                proposal_kind: SupervisorTurnProposalKind::ManualRefresh,
                proposed_text: Some(
                    "Continue under Orcas supervision for the assigned work unit.".to_string(),
                ),
                rationale_summary: "manual refresh requested from supervisor cli".to_string(),
                status: SupervisorTurnDecisionStatus::ProposedToHuman,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: Some("from supervisor cli".to_string()),
            };
            state
                .decisions
                .insert(decision.decision_id.clone(), decision.clone());
            Ok(ipc::SupervisorDecisionManualRefreshResponse { decision })
        }

        async fn supervisor_decision_approve_and_send(
            &self,
            params: &ipc::SupervisorDecisionApproveAndSendRequest,
        ) -> Result<ipc::SupervisorDecisionApproveAndSendResponse> {
            if let Some(error) = self.approve_error.lock().expect("approve error").clone() {
                bail!("{error}");
            }

            let mut state = self.state.lock().expect("state");
            let approved_text = state
                .decisions
                .get(&params.decision_id)
                .ok_or_else(|| anyhow!("unknown supervisor decision `{}`", params.decision_id))?
                .proposed_text
                .clone();
            let updated = {
                let decision = state
                    .decisions
                    .get_mut(&params.decision_id)
                    .ok_or_else(|| {
                        anyhow!("unknown supervisor decision `{}`", params.decision_id)
                    })?;
                if decision.status != SupervisorTurnDecisionStatus::ProposedToHuman {
                    bail!(
                        "supervisor decision `{}` is not pending human review",
                        params.decision_id
                    );
                }
                decision.status = SupervisorTurnDecisionStatus::Sent;
                decision.approved_at = Some(Utc::now());
                decision.sent_at = Some(Utc::now());
                decision.clone()
            };
            state.last_approved_text = approved_text;
            Ok(ipc::SupervisorDecisionApproveAndSendResponse { decision: updated })
        }

        async fn supervisor_decision_reject(
            &self,
            params: &ipc::SupervisorDecisionRejectRequest,
        ) -> Result<ipc::SupervisorDecisionRejectResponse> {
            let mut state = self.state.lock().expect("state");
            let decision = state
                .decisions
                .get_mut(&params.decision_id)
                .ok_or_else(|| anyhow!("unknown supervisor decision `{}`", params.decision_id))?;
            if decision.status != SupervisorTurnDecisionStatus::ProposedToHuman {
                bail!(
                    "supervisor decision `{}` is not pending human review",
                    params.decision_id
                );
            }
            decision.status = SupervisorTurnDecisionStatus::Rejected;
            decision.rejected_at = Some(Utc::now());
            Ok(ipc::SupervisorDecisionRejectResponse {
                decision: decision.clone(),
            })
        }
    }

    #[tokio::test]
    async fn cli_create_steer_with_authored_text() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");

        let decision = SupervisorService::codex_decision_propose_steer_with_backend(
            &backend,
            "thread-1",
            "stay within the current bounded test slice",
            None,
            None,
        )
        .await
        .expect("create steer decision");

        assert_eq!(decision.assignment_id, "assignment-1");
        assert_eq!(
            decision.proposed_text.as_deref(),
            Some("stay within the current bounded test slice")
        );
    }

    #[tokio::test]
    async fn cli_create_steer_rejects_empty_text() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");

        let error = SupervisorService::codex_decision_propose_steer_with_backend(
            &backend,
            "thread-1",
            "   \n\t  ",
            None,
            None,
        )
        .await
        .expect_err("empty steer text must fail");

        assert!(error.to_string().contains("steer text must not be empty"));
    }

    #[tokio::test]
    async fn cli_create_steer_rejects_unassigned_thread() {
        let backend = FakeSupervisorCodexBackend::default();

        let error = SupervisorService::codex_decision_propose_steer_with_backend(
            &backend,
            "thread-404",
            "continue the active turn",
            None,
            None,
        )
        .await
        .expect_err("unassigned thread must fail");

        assert!(
            error
                .to_string()
                .contains("no active Codex assignment for thread `thread-404`")
        );
    }

    #[tokio::test]
    async fn cli_create_steer_rejects_idle_thread_backend_error() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");
        *backend.propose_error.lock().expect("propose error") =
            Some("thread `thread-1` has no active turn".to_string());

        let error = SupervisorService::codex_decision_propose_steer_with_backend(
            &backend,
            "thread-1",
            "continue the active turn",
            None,
            None,
        )
        .await
        .expect_err("idle thread must fail");

        assert!(error.to_string().contains("no active turn"));
    }

    #[tokio::test]
    async fn cli_replace_pending_steer_with_authored_text_supersedes_old_revision() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");
        let original = SupervisorService::codex_decision_propose_steer_with_backend(
            &backend,
            "thread-1",
            "draft steer text",
            None,
            None,
        )
        .await
        .expect("create steer decision");

        let replacement = SupervisorService::codex_decision_replace_pending_steer_with_backend(
            &backend,
            &original.decision_id,
            "replacement steer text",
            None,
            None,
        )
        .await
        .expect("replace steer decision");

        let state = backend.state.lock().expect("state");
        let superseded = state
            .decisions
            .get(&original.decision_id)
            .expect("superseded decision");
        assert_eq!(superseded.status, SupervisorTurnDecisionStatus::Superseded);
        assert_eq!(
            superseded.superseded_by.as_deref(),
            Some(replacement.decision_id.as_str())
        );
        assert_eq!(
            replacement.proposed_text.as_deref(),
            Some("replacement steer text")
        );
    }

    #[tokio::test]
    async fn cli_replace_pending_steer_rejects_empty_text() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");
        let original = SupervisorService::codex_decision_propose_steer_with_backend(
            &backend,
            "thread-1",
            "draft steer text",
            None,
            None,
        )
        .await
        .expect("create steer decision");

        let error = SupervisorService::codex_decision_replace_pending_steer_with_backend(
            &backend,
            &original.decision_id,
            " \n ",
            None,
            None,
        )
        .await
        .expect_err("empty replacement must fail");

        assert!(error.to_string().contains("steer text must not be empty"));
    }

    #[tokio::test]
    async fn cli_cannot_replace_non_pending_steer_decisions() {
        let backend = FakeSupervisorCodexBackend::default();
        for status in [
            SupervisorTurnDecisionStatus::Sent,
            SupervisorTurnDecisionStatus::Rejected,
            SupervisorTurnDecisionStatus::Stale,
            SupervisorTurnDecisionStatus::Superseded,
        ] {
            let decision_id = format!("decision-{status:?}");
            seed_decision(
                &backend,
                sample_decision(
                    &decision_id,
                    "assignment-1",
                    "thread-1",
                    Some("turn-active-1"),
                    SupervisorTurnDecisionKind::SteerActiveTurn,
                    status,
                    Some("existing steer"),
                ),
            );

            let error = SupervisorService::codex_decision_replace_pending_steer_with_backend(
                &backend,
                &decision_id,
                "replacement steer text",
                None,
                None,
            )
            .await
            .expect_err("non-pending steer must fail");

            assert!(error.to_string().contains("no longer editable"));
        }
    }

    #[tokio::test]
    async fn cli_approve_and_send_uses_current_pending_steer_text() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");
        let original = SupervisorService::codex_decision_propose_steer_with_backend(
            &backend,
            "thread-1",
            "initial steer",
            None,
            None,
        )
        .await
        .expect("create steer decision");
        let replacement = SupervisorService::codex_decision_replace_pending_steer_with_backend(
            &backend,
            &original.decision_id,
            "replacement steer",
            None,
            None,
        )
        .await
        .expect("replace steer decision");

        let response = backend
            .supervisor_decision_approve_and_send(&ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: replacement.decision_id.clone(),
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect("approve replacement");

        assert_eq!(response.decision.status, SupervisorTurnDecisionStatus::Sent);
        let state = backend.state.lock().expect("state");
        assert_eq!(
            state.last_approved_text.as_deref(),
            Some("replacement steer")
        );
    }

    #[tokio::test]
    async fn cli_approve_and_send_reports_stale_basis_failures() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");
        let decision = SupervisorService::codex_decision_propose_steer_with_backend(
            &backend,
            "thread-1",
            "keep working",
            None,
            None,
        )
        .await
        .expect("create steer decision");
        *backend.approve_error.lock().expect("approve error") =
            Some("decision basis is stale: active turn changed".to_string());

        let error = backend
            .supervisor_decision_approve_and_send(&ipc::SupervisorDecisionApproveAndSendRequest {
                decision_id: decision.decision_id,
                reviewed_by: Some("reviewer".to_string()),
                review_note: None,
            })
            .await
            .expect_err("stale approval must fail");

        assert!(error.to_string().contains("active turn changed"));
    }

    #[tokio::test]
    async fn filtered_decision_listing_by_status_kind_thread_and_assignment() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_assignment_with_context(
            &backend,
            "assignment-1",
            "thread-1",
            "ws-1",
            "wu-1",
            "supervisor-a",
            CodexThreadAssignmentStatus::Active,
        );
        seed_assignment_with_context(
            &backend,
            "assignment-2",
            "thread-2",
            "ws-2",
            "wu-2",
            "supervisor-b",
            CodexThreadAssignmentStatus::Active,
        );
        seed_decision(
            &backend,
            SupervisorTurnDecision {
                decision_id: "std-pending-next".to_string(),
                assignment_id: "assignment-1".to_string(),
                codex_thread_id: "thread-1".to_string(),
                basis_turn_id: Some("turn-1".to_string()),
                kind: SupervisorTurnDecisionKind::NextTurn,
                proposal_kind: SupervisorTurnProposalKind::Bootstrap,
                proposed_text: Some("summarize status".to_string()),
                rationale_summary: "pending next turn".to_string(),
                status: SupervisorTurnDecisionStatus::ProposedToHuman,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: None,
            },
        );
        seed_decision(
            &backend,
            SupervisorTurnDecision {
                decision_id: "std-recorded".to_string(),
                assignment_id: "assignment-1".to_string(),
                codex_thread_id: "thread-1".to_string(),
                basis_turn_id: Some("turn-1".to_string()),
                kind: SupervisorTurnDecisionKind::NoAction,
                proposal_kind: SupervisorTurnProposalKind::Bootstrap,
                proposed_text: None,
                rationale_summary: "wait".to_string(),
                status: SupervisorTurnDecisionStatus::Recorded,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: None,
            },
        );
        seed_decision(
            &backend,
            SupervisorTurnDecision {
                decision_id: "std-steer".to_string(),
                assignment_id: "assignment-2".to_string(),
                codex_thread_id: "thread-2".to_string(),
                basis_turn_id: Some("turn-2".to_string()),
                kind: SupervisorTurnDecisionKind::SteerActiveTurn,
                proposal_kind: SupervisorTurnProposalKind::OperatorSteer,
                proposed_text: Some("focus logs".to_string()),
                rationale_summary: "pending steer".to_string(),
                status: SupervisorTurnDecisionStatus::ProposedToHuman,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: None,
            },
        );

        let by_status = SupervisorService::codex_decision_list_with_backend(
            &backend,
            None,
            None,
            None,
            None,
            None,
            Some(SupervisorTurnDecisionStatus::Recorded),
            None,
            true,
            true,
            false,
            None,
        )
        .await
        .expect("status filter");
        assert_eq!(by_status.len(), 1);
        assert_eq!(by_status[0].decision_id, "std-recorded");

        let by_kind = SupervisorService::codex_decision_list_with_backend(
            &backend,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(SupervisorTurnDecisionKind::SteerActiveTurn),
            true,
            true,
            false,
            None,
        )
        .await
        .expect("kind filter");
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].decision_id, "std-steer");

        let by_thread = SupervisorService::codex_decision_list_with_backend(
            &backend,
            Some("thread-1"),
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            false,
            None,
        )
        .await
        .expect("thread filter");
        assert_eq!(by_thread.len(), 2);
        assert!(
            by_thread
                .iter()
                .all(|decision| decision.codex_thread_id == "thread-1")
        );

        let by_assignment = SupervisorService::codex_decision_list_with_backend(
            &backend,
            None,
            Some("assignment-2"),
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            false,
            None,
        )
        .await
        .expect("assignment filter");
        assert_eq!(by_assignment.len(), 1);
        assert_eq!(by_assignment[0].decision_id, "std-steer");
    }

    #[tokio::test]
    async fn actionable_queue_excludes_non_pending_decisions_and_supports_workflow_filters() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_assignment_with_context(
            &backend,
            "assignment-1",
            "thread-1",
            "ws-1",
            "wu-1",
            "supervisor-a",
            CodexThreadAssignmentStatus::Active,
        );
        seed_assignment_with_context(
            &backend,
            "assignment-2",
            "thread-2",
            "ws-2",
            "wu-2",
            "supervisor-b",
            CodexThreadAssignmentStatus::Active,
        );
        seed_decision(
            &backend,
            sample_decision(
                "std-pending",
                "assignment-1",
                "thread-1",
                Some("turn-1"),
                SupervisorTurnDecisionKind::NextTurn,
                SupervisorTurnDecisionStatus::ProposedToHuman,
                Some("continue"),
            ),
        );
        seed_decision(
            &backend,
            SupervisorTurnDecision {
                decision_id: "std-recorded".to_string(),
                assignment_id: "assignment-1".to_string(),
                codex_thread_id: "thread-1".to_string(),
                basis_turn_id: Some("turn-1".to_string()),
                kind: SupervisorTurnDecisionKind::NoAction,
                proposal_kind: SupervisorTurnProposalKind::Bootstrap,
                proposed_text: None,
                rationale_summary: "wait".to_string(),
                status: SupervisorTurnDecisionStatus::Recorded,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: None,
            },
        );
        seed_decision(
            &backend,
            sample_decision(
                "std-other",
                "assignment-2",
                "thread-2",
                Some("turn-2"),
                SupervisorTurnDecisionKind::InterruptActiveTurn,
                SupervisorTurnDecisionStatus::ProposedToHuman,
                None,
            ),
        );

        let queue = SupervisorService::codex_decision_list_with_backend(
            &backend,
            None,
            None,
            Some("ws-1"),
            None,
            None,
            None,
            None,
            false,
            false,
            true,
            None,
        )
        .await
        .expect("actionable queue");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].decision_id, "std-pending");
        assert_eq!(queue[0].workstream_id.as_deref(), Some("ws-1"));
    }

    #[tokio::test]
    async fn history_view_includes_superseded_steer_revisions_and_no_action_entries() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_assignment_with_context(
            &backend,
            "assignment-1",
            "thread-1",
            "ws-1",
            "wu-1",
            "supervisor-a",
            CodexThreadAssignmentStatus::Active,
        );
        seed_decision(
            &backend,
            SupervisorTurnDecision {
                superseded_by: Some("std-2".to_string()),
                ..sample_decision(
                    "std-1",
                    "assignment-1",
                    "thread-1",
                    Some("turn-1"),
                    SupervisorTurnDecisionKind::SteerActiveTurn,
                    SupervisorTurnDecisionStatus::Superseded,
                    Some("first steer"),
                )
            },
        );
        seed_decision(
            &backend,
            sample_decision(
                "std-2",
                "assignment-1",
                "thread-1",
                Some("turn-1"),
                SupervisorTurnDecisionKind::SteerActiveTurn,
                SupervisorTurnDecisionStatus::ProposedToHuman,
                Some("second steer"),
            ),
        );
        seed_decision(
            &backend,
            SupervisorTurnDecision {
                decision_id: "std-wait".to_string(),
                assignment_id: "assignment-1".to_string(),
                codex_thread_id: "thread-1".to_string(),
                basis_turn_id: Some("turn-2".to_string()),
                kind: SupervisorTurnDecisionKind::NoAction,
                proposal_kind: SupervisorTurnProposalKind::ManualRefresh,
                proposed_text: None,
                rationale_summary: "wait on current basis".to_string(),
                status: SupervisorTurnDecisionStatus::Recorded,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: None,
            },
        );

        let history = SupervisorService::codex_decision_list_with_backend(
            &backend,
            Some("thread-1"),
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            false,
            None,
        )
        .await
        .expect("history");
        let rendered = history
            .iter()
            .map(SupervisorService::format_supervisor_turn_decision_summary)
            .collect::<Vec<_>>()
            .join("\n");
        let chains = SupervisorService::format_decision_revision_chains(&history).join("\n");

        assert!(rendered.contains("std-1"));
        assert!(rendered.contains("std-2"));
        assert!(rendered.contains("std-wait"));
        assert!(rendered.contains("superseded_by=std-2"));
        assert!(chains.contains("std-1 -> std-2"));
    }

    #[tokio::test]
    async fn queue_and_get_formatting_include_operator_useful_fields() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_assignment_with_context(
            &backend,
            "assignment-1",
            "thread-1",
            "ws-1",
            "wu-1",
            "supervisor-a",
            CodexThreadAssignmentStatus::Active,
        );
        let decision = sample_decision(
            "std-queue",
            "assignment-1",
            "thread-1",
            Some("turn-1"),
            SupervisorTurnDecisionKind::NextTurn,
            SupervisorTurnDecisionStatus::ProposedToHuman,
            Some("Continue with the next bounded step."),
        );
        seed_decision(&backend, decision.clone());
        let listed = SupervisorService::codex_decision_list_with_backend(
            &backend,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            false,
            Some(10),
        )
        .await
        .expect("list");
        let summary_line = SupervisorService::format_supervisor_turn_decision_summary(&listed[0]);
        assert!(summary_line.contains("thread=thread-1"));
        assert!(summary_line.contains("assignment=assignment-1"));
        assert!(summary_line.contains("ws=ws-1"));
        assert!(summary_line.contains("wu=wu-1"));
        assert!(summary_line.contains("supervisor=supervisor-a"));
        assert!(summary_line.contains("rationale="));
        assert!(summary_line.contains("text=Continue with the next bounded step."));

        let detail_lines =
            SupervisorService::format_supervisor_turn_decision(&decision, listed.first(), &listed)
                .join("\n");
        assert!(detail_lines.contains("workstream_id: ws-1"));
        assert!(detail_lines.contains("work_unit_id: wu-1"));
        assert!(detail_lines.contains("supervisor_id: supervisor-a"));
        assert!(detail_lines.contains("actionable: yes"));
        assert!(detail_lines.contains("related_history:"));
    }

    #[tokio::test]
    async fn approve_and_reject_continue_to_work_with_decisions_surfaced_from_queue() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");
        let pending = sample_decision(
            "std-pending",
            "assignment-1",
            "thread-1",
            Some("turn-1"),
            SupervisorTurnDecisionKind::NextTurn,
            SupervisorTurnDecisionStatus::ProposedToHuman,
            Some("continue"),
        );
        seed_decision(&backend, pending.clone());

        let queue = SupervisorService::codex_decision_list_with_backend(
            &backend, None, None, None, None, None, None, None, false, false, true, None,
        )
        .await
        .expect("queue");
        let approved = SupervisorService::codex_decision_approve_and_send_with_backend(
            &backend,
            &queue[0].decision_id,
            None,
            None,
        )
        .await
        .expect("approve");
        assert_eq!(approved.status, SupervisorTurnDecisionStatus::Sent);

        let replacement = sample_decision(
            "std-pending-reject",
            "assignment-1",
            "thread-1",
            Some("turn-2"),
            SupervisorTurnDecisionKind::NextTurn,
            SupervisorTurnDecisionStatus::ProposedToHuman,
            Some("continue"),
        );
        seed_decision(&backend, replacement.clone());
        let queue = SupervisorService::codex_decision_list_with_backend(
            &backend, None, None, None, None, None, None, None, false, false, true, None,
        )
        .await
        .expect("queue after replacement");
        let rejected = SupervisorService::codex_decision_reject_with_backend(
            &backend,
            &queue[0].decision_id,
            None,
            None,
        )
        .await
        .expect("reject");
        assert_eq!(rejected.status, SupervisorTurnDecisionStatus::Rejected);
    }

    #[tokio::test]
    async fn cli_record_no_action_from_pending_next_turn() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");
        seed_decision(
            &backend,
            SupervisorTurnDecision {
                decision_id: "std-next".to_string(),
                assignment_id: "assignment-1".to_string(),
                codex_thread_id: "thread-1".to_string(),
                basis_turn_id: Some("turn-1".to_string()),
                kind: SupervisorTurnDecisionKind::NextTurn,
                proposal_kind: SupervisorTurnProposalKind::Bootstrap,
                proposed_text: Some("summarize status".to_string()),
                rationale_summary: "pending next-turn review".to_string(),
                status: SupervisorTurnDecisionStatus::ProposedToHuman,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: None,
            },
        );

        let decision = SupervisorService::codex_decision_record_no_action_with_backend(
            &backend,
            "std-next",
            None,
            Some("wait for more context".to_string()),
        )
        .await
        .expect("record no_action");

        assert_eq!(decision.kind, SupervisorTurnDecisionKind::NoAction);
        assert_eq!(decision.status, SupervisorTurnDecisionStatus::Recorded);

        let state = backend.state.lock().expect("state");
        let previous = state.decisions.get("std-next").expect("previous decision");
        assert_eq!(previous.status, SupervisorTurnDecisionStatus::Superseded);
        assert_eq!(
            previous.superseded_by.as_deref(),
            Some(decision.decision_id.as_str())
        );
    }

    #[tokio::test]
    async fn cli_manual_refresh_creates_pending_next_turn_from_recorded_no_action() {
        let backend = FakeSupervisorCodexBackend::default();
        seed_active_assignment(&backend, "assignment-1", "thread-1");
        seed_decision(
            &backend,
            SupervisorTurnDecision {
                decision_id: "std-wait".to_string(),
                assignment_id: "assignment-1".to_string(),
                codex_thread_id: "thread-1".to_string(),
                basis_turn_id: Some("turn-1".to_string()),
                kind: SupervisorTurnDecisionKind::NoAction,
                proposal_kind: SupervisorTurnProposalKind::Bootstrap,
                proposed_text: None,
                rationale_summary: "wait on the current basis".to_string(),
                status: SupervisorTurnDecisionStatus::Recorded,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: Some("operator chose not to send".to_string()),
            },
        );

        let decision = SupervisorService::codex_decision_manual_refresh_with_backend(
            &backend,
            Some("thread-1"),
            None,
            None,
            Some("check again now".to_string()),
        )
        .await
        .expect("manual refresh");

        assert_eq!(decision.kind, SupervisorTurnDecisionKind::NextTurn);
        assert_eq!(
            decision.proposal_kind,
            SupervisorTurnProposalKind::ManualRefresh
        );
        assert_eq!(
            decision.status,
            SupervisorTurnDecisionStatus::ProposedToHuman
        );
    }

    #[tokio::test]
    async fn cli_manual_refresh_rejects_missing_target_selection() {
        let backend = FakeSupervisorCodexBackend::default();

        let error = SupervisorService::codex_decision_manual_refresh_with_backend(
            &backend, None, None, None, None,
        )
        .await
        .expect_err("manual refresh requires thread or assignment");

        assert!(
            error
                .to_string()
                .contains("manual refresh requires --thread or --assignment")
        );
    }

    #[tokio::test]
    async fn cli_output_surfaces_no_action_and_manual_refresh_details() {
        let lines = SupervisorService::format_supervisor_turn_decision(
            &SupervisorTurnDecision {
                decision_id: "std-wait".to_string(),
                assignment_id: "assignment-1".to_string(),
                codex_thread_id: "thread-1".to_string(),
                basis_turn_id: Some("turn-1".to_string()),
                kind: SupervisorTurnDecisionKind::NoAction,
                proposal_kind: SupervisorTurnProposalKind::ManualRefresh,
                proposed_text: None,
                rationale_summary: "operator deliberately chose to wait".to_string(),
                status: SupervisorTurnDecisionStatus::Recorded,
                created_at: Utc::now(),
                approved_at: None,
                rejected_at: None,
                sent_at: None,
                superseded_by: None,
                sent_turn_id: None,
                notes: Some("waiting on current basis".to_string()),
            },
            None,
            &[],
        );
        let rendered = lines.join("\n");

        assert!(rendered.contains("kind: NoAction"));
        assert!(rendered.contains("proposal_kind: ManualRefresh"));
        assert!(rendered.contains("status: Recorded"));
        assert!(rendered.contains("basis_turn_id: turn-1"));
        assert!(rendered.contains("notes: waiting on current basis"));
    }

    #[test]
    fn cli_output_surfaces_steer_decision_details() {
        let decision = sample_decision(
            "std-9",
            "assignment-1",
            "thread-1",
            Some("turn-active-1"),
            SupervisorTurnDecisionKind::SteerActiveTurn,
            SupervisorTurnDecisionStatus::ProposedToHuman,
            Some("first line\nsecond line"),
        );

        let lines = SupervisorService::format_supervisor_turn_decision(&decision, None, &[]);
        let rendered = lines.join("\n");

        assert!(rendered.contains("decision_id: std-9"));
        assert!(rendered.contains("thread_id: thread-1"));
        assert!(rendered.contains("kind: SteerActiveTurn"));
        assert!(rendered.contains("status: ProposedToHuman"));
        assert!(rendered.contains("basis_turn_id: turn-active-1"));
        assert!(rendered.contains("rationale_summary: operator supplied steer guidance"));
        assert!(rendered.contains("proposed_text:"));
        assert!(rendered.contains("  first line"));
        assert!(rendered.contains("  second line"));
    }

    #[test]
    fn cli_decision_list_summary_surfaces_supersession_chain() {
        let summary = sample_decision_summary(
            &SupervisorTurnDecision {
                superseded_by: Some("std-2".to_string()),
                ..sample_decision(
                    "std-1",
                    "assignment-1",
                    "thread-1",
                    Some("turn-active-1"),
                    SupervisorTurnDecisionKind::SteerActiveTurn,
                    SupervisorTurnDecisionStatus::Superseded,
                    Some("first line\nsecond line"),
                )
            },
            None,
        );

        let rendered = SupervisorService::format_supervisor_turn_decision_summary(&summary);

        assert!(rendered.contains("std-1"));
        assert!(rendered.contains("Superseded"));
        assert!(rendered.contains("superseded_by=std-2"));
        assert!(rendered.contains("first line second line"));
    }

    fn seed_active_assignment(
        backend: &FakeSupervisorCodexBackend,
        assignment_id: &str,
        thread_id: &str,
    ) {
        seed_assignment_with_context(
            backend,
            assignment_id,
            thread_id,
            "workstream-1",
            "work-unit-1",
            "supervisor-1",
            CodexThreadAssignmentStatus::Active,
        );
    }

    fn seed_assignment_with_context(
        backend: &FakeSupervisorCodexBackend,
        assignment_id: &str,
        thread_id: &str,
        workstream_id: &str,
        work_unit_id: &str,
        supervisor_id: &str,
        status: CodexThreadAssignmentStatus,
    ) {
        backend
            .state
            .lock()
            .expect("state")
            .assignments
            .push(ipc::CodexThreadAssignmentSummary {
                assignment_id: assignment_id.to_string(),
                codex_thread_id: thread_id.to_string(),
                workstream_id: workstream_id.to_string(),
                work_unit_id: work_unit_id.to_string(),
                supervisor_id: supervisor_id.to_string(),
                assigned_by: "tester".to_string(),
                assigned_at: Utc::now(),
                updated_at: Utc::now(),
                status,
                send_policy: CodexThreadSendPolicy::HumanApprovalRequired,
                bootstrap_state: CodexThreadBootstrapState::Sent,
                latest_basis_turn_id: Some("turn-active-1".to_string()),
                latest_decision_id: None,
                notes: None,
                active: status == CodexThreadAssignmentStatus::Active,
            });
    }

    fn seed_decision(backend: &FakeSupervisorCodexBackend, decision: SupervisorTurnDecision) {
        backend
            .state
            .lock()
            .expect("state")
            .decisions
            .insert(decision.decision_id.clone(), decision);
    }

    fn sample_decision(
        decision_id: &str,
        assignment_id: &str,
        codex_thread_id: &str,
        basis_turn_id: Option<&str>,
        kind: SupervisorTurnDecisionKind,
        status: SupervisorTurnDecisionStatus,
        proposed_text: Option<&str>,
    ) -> SupervisorTurnDecision {
        SupervisorTurnDecision {
            decision_id: decision_id.to_string(),
            assignment_id: assignment_id.to_string(),
            codex_thread_id: codex_thread_id.to_string(),
            basis_turn_id: basis_turn_id.map(ToOwned::to_owned),
            kind,
            proposal_kind: SupervisorTurnProposalKind::OperatorSteer,
            proposed_text: proposed_text.map(ToOwned::to_owned),
            rationale_summary: "operator supplied steer guidance".to_string(),
            status,
            created_at: Utc::now(),
            approved_at: None,
            rejected_at: None,
            sent_at: None,
            superseded_by: None,
            sent_turn_id: None,
            notes: Some("from supervisor cli".to_string()),
        }
    }

    fn assignment_summary_for_decision<'a>(
        assignments: &'a [ipc::CodexThreadAssignmentSummary],
        decision: &SupervisorTurnDecision,
    ) -> Option<&'a ipc::CodexThreadAssignmentSummary> {
        assignments
            .iter()
            .find(|assignment| assignment.assignment_id == decision.assignment_id)
    }

    fn sample_decision_summary(
        decision: &SupervisorTurnDecision,
        assignment: Option<&ipc::CodexThreadAssignmentSummary>,
    ) -> ipc::SupervisorTurnDecisionSummary {
        ipc::SupervisorTurnDecisionSummary {
            decision_id: decision.decision_id.clone(),
            assignment_id: decision.assignment_id.clone(),
            codex_thread_id: decision.codex_thread_id.clone(),
            workstream_id: assignment.map(|assignment| assignment.workstream_id.clone()),
            work_unit_id: assignment.map(|assignment| assignment.work_unit_id.clone()),
            supervisor_id: assignment.map(|assignment| assignment.supervisor_id.clone()),
            basis_turn_id: decision.basis_turn_id.clone(),
            kind: decision.kind,
            proposal_kind: decision.proposal_kind,
            proposed_text: decision.proposed_text.clone(),
            rationale_summary: decision.rationale_summary.clone(),
            status: decision.status,
            created_at: decision.created_at,
            approved_at: decision.approved_at,
            rejected_at: decision.rejected_at,
            sent_at: decision.sent_at,
            superseded_by: decision.superseded_by.clone(),
            sent_turn_id: decision.sent_turn_id.clone(),
            notes: decision.notes.clone(),
            open: decision.status == SupervisorTurnDecisionStatus::ProposedToHuman,
        }
    }

    fn sample_proposal_artifact_export(
        status: SupervisorProposalStatus,
        failure: Option<SupervisorProposalFailure>,
    ) -> ipc::SupervisorProposalArtifactExport {
        ipc::SupervisorProposalArtifactExport {
            proposal_id: "proposal-1".to_string(),
            primary_work_unit_id: "wu-1".to_string(),
            source_report_id: "report-1".to_string(),
            proposal_status: status,
            created_at: Utc::now(),
            validated_at: Some(Utc::now()),
            reviewed_at: Some(Utc::now()),
            reviewed_by: Some("reviewer".to_string()),
            review_note: Some("review note".to_string()),
            approved_decision_id: Some("decision-1".to_string()),
            approved_assignment_id: Some("assignment-2".to_string()),
            artifact_summary: ipc::SupervisorProposalArtifactSummary {
                proposal_id: "proposal-1".to_string(),
                proposal_status: status,
                prompt_artifact_present: true,
                prompt_template_version: Some("supervisor_prompt.v1".to_string()),
                prompt_hash: Some("prompt-hash".to_string()),
                request_body_hash: Some("request-hash".to_string()),
                response_artifact_present: true,
                response_hash: Some("response-hash".to_string()),
                raw_response_body_present: true,
                raw_response_body_hash: Some("raw-hash".to_string()),
                reasoner_backend: "test".to_string(),
                reasoner_model: "test-model".to_string(),
                reasoner_response_id: Some("resp-1".to_string()),
                parsed_proposal_present: failure.is_none(),
                approved_proposal_present: status == SupervisorProposalStatus::Approved,
                generation_failure_stage: failure.as_ref().map(|value| value.stage),
            },
            artifact_detail: ipc::SupervisorProposalArtifactDetail {
                proposal_id: "proposal-1".to_string(),
                proposal_status: status,
                created_at: Utc::now(),
                validated_at: Some(Utc::now()),
                reviewed_at: Some(Utc::now()),
                reasoner_backend: "test".to_string(),
                reasoner_model: "test-model".to_string(),
                reasoner_response_id: Some("resp-1".to_string()),
                prompt_render: Some(SupervisorPromptRenderArtifact {
                    render_spec: SupervisorPromptRenderSpec {
                        template_version: "supervisor_prompt.v1".to_string(),
                        context_schema_version: "supervisor_context.v1".to_string(),
                        proposal_schema_name: "supervisor_proposal".to_string(),
                        proposal_schema_version: "supervisor_proposal.v1".to_string(),
                        response_format: "json_schema".to_string(),
                        strict_schema: true,
                        context_serialization: "json_pretty".to_string(),
                        style: "plain_text_markdown".to_string(),
                    },
                    instructions_text: "You are the Orcas supervisor reasoner.".to_string(),
                    user_content_text: "SupervisorContextPack:\n{}".to_string(),
                    context_pack_text: "{\n  \"schema_version\": \"supervisor_context.v1\"\n}"
                        .to_string(),
                    prompt_hash: "prompt-hash".to_string(),
                    request_body_hash: Some("request-hash".to_string()),
                    rendered_at: Utc::now(),
                }),
                response_artifact: Some(SupervisorResponseArtifact {
                    backend_kind: "test".to_string(),
                    model: "test-model".to_string(),
                    response_id: Some("resp-1".to_string()),
                    usage: Some(SupervisorReasonerUsage {
                        input_tokens: Some(10),
                        output_tokens: Some(20),
                        total_tokens: Some(30),
                    }),
                    output_items: vec![SupervisorResponseOutputItem {
                        item_type: "message".to_string(),
                        role: Some("assistant".to_string()),
                        status: Some("completed".to_string()),
                        content: vec![SupervisorResponseContentPart {
                            part_type: "output_text".to_string(),
                            text: Some(
                                "{\"schema_version\":\"supervisor_proposal.v1\"}".to_string(),
                            ),
                        }],
                    }],
                    extracted_output_text: Some(
                        "{\"schema_version\":\"supervisor_proposal.v1\"}".to_string(),
                    ),
                    response_hash: "response-hash".to_string(),
                    raw_response_body: Some("{\"id\":\"resp-1\"}".to_string()),
                    raw_response_body_hash: Some("raw-hash".to_string()),
                    captured_at: Utc::now(),
                }),
                reasoner_output_text: Some(
                    "{\"schema_version\":\"supervisor_proposal.v1\"}".to_string(),
                ),
                parsed_proposal: None,
                approved_proposal: None,
                generation_failure: failure,
            },
        }
    }

    #[test]
    fn proposal_artifact_export_json_is_lossless() {
        let export = sample_proposal_artifact_export(SupervisorProposalStatus::Open, None);
        let rendered =
            SupervisorService::render_proposal_artifact_export_json(&export).expect("render json");
        let round_trip: ipc::SupervisorProposalArtifactExport =
            serde_json::from_str(&rendered).expect("parse export json");

        assert_eq!(round_trip.proposal_id, export.proposal_id);
        assert_eq!(
            round_trip.artifact_summary.prompt_hash,
            export.artifact_summary.prompt_hash
        );
        assert_eq!(
            round_trip.artifact_detail.reasoner_output_text,
            export.artifact_detail.reasoner_output_text
        );
        assert!(rendered.contains("\"prompt_render\""));
        assert!(rendered.contains("\"response_artifact\""));
    }

    #[test]
    fn proposal_artifact_export_markdown_is_structured() {
        let export = sample_proposal_artifact_export(
            SupervisorProposalStatus::GenerationFailed,
            Some(SupervisorProposalFailure {
                stage: SupervisorProposalFailureStage::ProposalMalformed,
                message: "failed to decode supervisor proposal JSON".to_string(),
            }),
        );
        let rendered = SupervisorService::render_proposal_artifact_export_markdown(&export)
            .expect("render markdown");

        assert!(rendered.contains("# Supervisor Proposal Artifact Export"));
        assert!(rendered.contains("## Proposal Metadata"));
        assert!(rendered.contains("## Artifact Summary"));
        assert!(rendered.contains("## Prompt Artifact"));
        assert!(rendered.contains("## Response Artifact"));
        assert!(rendered.contains("## Extracted Output Text"));
        assert!(rendered.contains("## Failure Metadata"));
        assert!(rendered.contains("prompt-hash"));
        assert!(rendered.contains("response-hash"));
        assert!(rendered.contains("ProposalMalformed"));
    }
}
