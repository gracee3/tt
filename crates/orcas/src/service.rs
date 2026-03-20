use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::{Error, Result, anyhow, bail};
use async_trait::async_trait;
use tokio::time::sleep;
use tracing::{info, warn};

use orcas_core::{
    AppConfig, AppPaths, CodexThreadAssignmentStatus, DecisionType, SupervisorProposalEdits,
    SupervisorProposalRecord, SupervisorTurnDecision, SupervisorTurnDecisionKind,
    SupervisorTurnDecisionStatus, ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest,
    WorkUnitStatus, WorkstreamStatus, authority, ipc,
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
        for thread in response.data {
            println!(
                "{}\t{}\t{}\t{}\tin_flight={}\t{}\t{}",
                thread.id,
                thread.status,
                thread.model_provider,
                thread.scope,
                thread.turn_in_flight,
                thread
                    .recent_output
                    .clone()
                    .unwrap_or_else(|| thread.preview.replace('\n', " ")),
                thread.recent_event.unwrap_or_default()
            );
        }
        Ok(())
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
    ) -> Result<()> {
        let client = self.daemon_state_client().await?;
        let response = client
            .authority_tracked_thread_create(&ipc::AuthorityTrackedThreadCreateRequest {
                command: authority::CreateTrackedThread {
                    metadata: Self::authority_command_metadata(),
                    tracked_thread_id: authority::TrackedThreadId::new(),
                    work_unit_id: authority::WorkUnitId::parse(work_unit_id.to_string())?,
                    title,
                    notes,
                    backend_kind: authority::TrackedThreadBackendKind::Codex,
                    upstream_thread_id,
                    preferred_cwd: Some(root_dir),
                    preferred_model,
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
        CodexThreadBootstrapState, CodexThreadSendPolicy, SupervisorTurnProposalKind,
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
}
