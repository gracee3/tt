use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Error, Result};
use tokio::time::sleep;

use orcas_core::{
    AppConfig, AppPaths, DecisionType, SupervisorProposalEdits, SupervisorProposalRecord,
    ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest, ipc,
};
use orcas_daemon::{
    OrcasDaemonLaunch, OrcasDaemonProcessManager, OrcasIpcClient, OrcasRuntimeOverrides,
    apply_runtime_overrides,
};

use crate::streaming::{
    ConsoleReporter, OrcasSupervisorStreamingBackend, RetryPolicy, StreamReporter,
    StreamingCommandRunner,
};

pub use orcas_daemon::OrcasRuntimeOverrides as RuntimeOverrides;

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
        let client = self.ready_client().await?;
        let response = client
            .workstream_create(&ipc::WorkstreamCreateRequest {
                title,
                objective,
                priority,
            })
            .await?;
        println!("workstream_id: {}", response.workstream.id);
        println!("status: {:?}", response.workstream.status);
        Ok(())
    }

    pub async fn workstream_list(&self) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client.workstream_list().await?;
        for workstream in response.workstreams {
            println!(
                "{}\t{:?}\t{}\t{}",
                workstream.id, workstream.status, workstream.priority, workstream.title
            );
        }
        Ok(())
    }

    pub async fn workstream_get(&self, workstream_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .workstream_get(&ipc::WorkstreamGetRequest {
                workstream_id: workstream_id.to_string(),
            })
            .await?;
        println!("workstream_id: {}", response.workstream.id);
        println!("title: {}", response.workstream.title);
        println!("objective: {}", response.workstream.objective);
        println!("status: {:?}", response.workstream.status);
        println!("priority: {}", response.workstream.priority);
        println!("work_units: {}", response.work_units.len());
        for work_unit in response.work_units {
            println!(
                "work_unit\t{}\t{:?}\tdeps={}\t{}",
                work_unit.id,
                work_unit.status,
                work_unit.dependencies.len(),
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
        let client = self.ready_client().await?;
        let response = client
            .workunit_create(&ipc::WorkunitCreateRequest {
                workstream_id: workstream_id.to_string(),
                title,
                task_statement,
                dependencies,
            })
            .await?;
        println!("work_unit_id: {}", response.work_unit.id);
        println!("status: {:?}", response.work_unit.status);
        Ok(())
    }

    pub async fn workunit_list(&self, workstream_id: Option<&str>) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .workunit_list(&ipc::WorkunitListRequest {
                workstream_id: workstream_id.map(ToOwned::to_owned),
            })
            .await?;
        for work_unit in response.work_units {
            println!(
                "{}\t{:?}\tdeps={}\treport={}\t{}",
                work_unit.id,
                work_unit.status,
                work_unit.dependencies.len(),
                work_unit.latest_report_id.unwrap_or_default(),
                work_unit.title
            );
        }
        Ok(())
    }

    pub async fn workunit_get(&self, work_unit_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .workunit_get(&ipc::WorkunitGetRequest {
                work_unit_id: work_unit_id.to_string(),
            })
            .await?;
        println!("work_unit_id: {}", response.work_unit.id);
        println!("workstream_id: {}", response.work_unit.workstream_id);
        println!("title: {}", response.work_unit.title);
        println!("task_statement: {}", response.work_unit.task_statement);
        println!("status: {:?}", response.work_unit.status);
        println!(
            "dependencies: {}",
            if response.work_unit.dependencies.is_empty() {
                String::new()
            } else {
                response.work_unit.dependencies.join(",")
            }
        );
        if let Some(assignment_id) = response.work_unit.current_assignment_id.as_ref() {
            println!("current_assignment_id: {assignment_id}");
        }
        if let Some(report_id) = response.work_unit.latest_report_id.as_ref() {
            println!("latest_report_id: {report_id}");
        }
        println!("assignments: {}", response.assignments.len());
        println!("reports: {}", response.reports.len());
        println!("decisions: {}", response.decisions.len());
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
        let client = self.ready_client().await?;
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
        let client = self.ready_client().await?;
        let response = client
            .assignment_communication_get(&ipc::AssignmentCommunicationGetRequest {
                assignment_id: assignment_id.to_string(),
            })
            .await?;
        println!("{}", serde_json::to_string_pretty(&response.record)?);
        Ok(())
    }

    pub async fn report_get(&self, report_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
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
        let client = self.ready_client().await?;
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
        let client = self.ready_client().await?;
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
        let client = self.ready_client().await?;
        let response = client
            .proposal_create(&ipc::ProposalCreateRequest {
                work_unit_id: work_unit_id.to_string(),
                source_report_id,
                requested_by,
                note,
                supersede_open,
            })
            .await?;
        Self::print_proposal_record(&response.proposal);
        Ok(())
    }

    pub async fn proposal_get(&self, proposal_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .proposal_get(&ipc::ProposalGetRequest {
                proposal_id: proposal_id.to_string(),
            })
            .await?;
        Self::print_proposal_record(&response.proposal);
        Ok(())
    }

    pub async fn proposal_list_for_workunit(&self, work_unit_id: &str) -> Result<()> {
        let client = self.ready_client().await?;
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
        let client = self.ready_client().await?;
        let response = client
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
            .await?;

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
        Ok(())
    }

    pub async fn proposal_reject(
        &self,
        proposal_id: &str,
        reviewed_by: Option<String>,
        review_note: Option<String>,
    ) -> Result<()> {
        let client = self.ready_client().await?;
        let response = client
            .proposal_reject(&ipc::ProposalRejectRequest {
                proposal_id: proposal_id.to_string(),
                reviewed_by,
                review_note,
            })
            .await?;
        Self::print_proposal_record(&response.proposal);
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
