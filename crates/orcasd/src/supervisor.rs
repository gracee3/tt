use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::assignment_comm::{json_fingerprint, stable_fingerprint};
use orcas_core::supervisor::{
    DecisionPolicy, DraftAssignment, PriorDecisionContext, PriorReportContext,
    RecentPrimaryHistory, RelatedWorkUnitContext, SupervisorArtifactRef,
    SupervisorAssignmentContext, SupervisorContextPack, SupervisorDependencyContext,
    SupervisorDependencyContextItem, SupervisorOperatorRequest, SupervisorPackLimits,
    SupervisorPackTruncation, SupervisorPromptRenderArtifact, SupervisorPromptRenderSpec,
    SupervisorProposal, SupervisorProposalEdits, SupervisorProposalFailureStage,
    SupervisorProposalRecord, SupervisorResponseArtifact, SupervisorResponseContentPart,
    SupervisorResponseOutputItem, SupervisorSourceReportContext, SupervisorStateAnchor,
    SupervisorWorkUnitContext, SupervisorWorkerSessionContext, SupervisorWorkstreamContext,
    SupervisorWorkstreamPlanContext,
};
use orcas_core::{
    AppConfig, Assignment, CollaborationState, Decision, DecisionType, OrcasError, OrcasResult,
    Report, ReportDisposition, ReportParseResult, SupervisorProposalTrigger,
    SupervisorProposalTriggerKind, SupervisorReasonerUsage, WorkUnit, WorkUnitStatus,
    WorkerSession,
};

const CONTEXT_SCHEMA_VERSION: &str = "supervisor_context_pack.v2";
const PROPOSAL_SCHEMA_VERSION: &str = "supervisor_proposal.v2";
pub const SUPERVISOR_PROMPT_TEMPLATE_VERSION: &str = "supervisor_prompt.v1";
const SUPERVISOR_PROPOSAL_SCHEMA_NAME: &str = "supervisor_proposal";
const SUPERVISOR_PROMPT_STYLE: &str = "instructions_plus_json_context";
const SUPERVISOR_CONTEXT_SERIALIZATION: &str = "json_pretty";
const SUPERVISOR_RESPONSE_FORMAT: &str = "json_schema";
const EXPECTED_REPORT_FIELDS: &[&str] = &[
    "summary",
    "findings",
    "blockers",
    "questions",
    "recommended_next_actions",
    "confidence",
];

#[derive(Debug, Clone)]
pub struct SupervisorReasonerResult {
    pub proposal: SupervisorProposal,
    pub backend_kind: String,
    pub model: String,
    pub response_id: Option<String>,
    pub usage: Option<SupervisorReasonerUsage>,
    pub output_text: Option<String>,
    pub prompt_render: SupervisorPromptRenderArtifact,
    pub response_artifact: SupervisorResponseArtifact,
}

#[derive(Debug, Clone)]
pub struct SupervisorReasonerFailure {
    pub stage: SupervisorProposalFailureStage,
    pub message: String,
    pub backend_kind: String,
    pub model: String,
    pub response_id: Option<String>,
    pub output_text: Option<String>,
    pub prompt_render: Option<SupervisorPromptRenderArtifact>,
    pub response_artifact: Option<SupervisorResponseArtifact>,
}

#[async_trait]
pub trait SupervisorReasoner: Send + Sync {
    async fn propose(
        &self,
        pack: SupervisorContextPack,
    ) -> Result<SupervisorReasonerResult, SupervisorReasonerFailure>;
}

#[derive(Debug)]
pub struct ResponsesApiReasoner {
    client: Client,
    config: AppConfig,
}

impl ResponsesApiReasoner {
    pub fn new(config: AppConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    fn api_key(&self) -> OrcasResult<String> {
        std::env::var(&self.config.supervisor.api_key_env).map_err(|_| {
            OrcasError::Config(format!(
                "supervisor API key environment variable `{}` is not set",
                self.config.supervisor.api_key_env
            ))
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/responses",
            self.config.supervisor.base_url.trim_end_matches('/')
        )
    }

    fn request_body(
        &self,
        prompt_render: &SupervisorPromptRenderArtifact,
    ) -> OrcasResult<(Value, String)> {
        let body = json!({
            "model": self.config.supervisor.model,
            "store": false,
            "max_output_tokens": self.config.supervisor.max_output_tokens,
            "reasoning": {
                "effort": self.config.supervisor.reasoning_effort,
            },
            "instructions": prompt_render.instructions_text,
            "input": [{
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": prompt_render.user_content_text,
                }],
            }],
            "text": {
                "format": {
                    "type": prompt_render.render_spec.response_format,
                    "strict": prompt_render.render_spec.strict_schema,
                    "name": prompt_render.render_spec.proposal_schema_name,
                    "schema": proposal_json_schema(),
                }
            }
        });
        let request_body_hash = json_fingerprint(&body)?;
        Ok((body, request_body_hash))
    }

    fn failure(
        &self,
        stage: SupervisorProposalFailureStage,
        message: impl Into<String>,
        response_id: Option<String>,
        output_text: Option<String>,
        prompt_render: Option<SupervisorPromptRenderArtifact>,
        response_artifact: Option<SupervisorResponseArtifact>,
    ) -> SupervisorReasonerFailure {
        SupervisorReasonerFailure {
            stage,
            message: message.into(),
            backend_kind: "responses_api".to_string(),
            model: self.config.supervisor.model.clone(),
            response_id,
            output_text,
            prompt_render,
            response_artifact,
        }
    }
}

#[async_trait]
impl SupervisorReasoner for ResponsesApiReasoner {
    async fn propose(
        &self,
        pack: SupervisorContextPack,
    ) -> Result<SupervisorReasonerResult, SupervisorReasonerFailure> {
        let started_at = Instant::now();
        info!(
            work_unit_id = %pack.primary_work_unit.id,
            source_report_id = %pack.source_report.id,
            trigger_kind = ?pack.trigger.kind,
            backend_kind = "responses_api",
            model = %self.config.supervisor.model,
            "starting supervisor proposal generation"
        );
        let prompt_render = render_supervisor_prompt(&pack, Utc::now()).map_err(|error| {
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "render_prompt",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "supervisor proposal generation failed"
            );
            self.failure(
                SupervisorProposalFailureStage::Backend,
                error.to_string(),
                None,
                None,
                None,
                None,
            )
        })?;
        let api_key = self.api_key().map_err(|error| {
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "resolve_api_key",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "supervisor proposal generation failed"
            );
            self.failure(
                SupervisorProposalFailureStage::Backend,
                error.to_string(),
                None,
                None,
                Some(prompt_render.clone()),
                None,
            )
        })?;
        let (body, request_body_hash) = self.request_body(&prompt_render).map_err(|error| {
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "build_request",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "supervisor proposal generation failed"
            );
            self.failure(
                SupervisorProposalFailureStage::Backend,
                error.to_string(),
                None,
                None,
                Some(prompt_render.clone()),
                None,
            )
        })?;
        let prompt_render = SupervisorPromptRenderArtifact {
            request_body_hash: Some(request_body_hash),
            ..prompt_render
        };
        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                warn!(
                    work_unit_id = %pack.primary_work_unit.id,
                    source_report_id = %pack.source_report.id,
                    stage = "send_request",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    error = %error,
                    "supervisor proposal generation failed"
                );
                self.failure(
                    SupervisorProposalFailureStage::Backend,
                    format!("supervisor Responses API request failed: {error}"),
                    None,
                    None,
                    Some(prompt_render.clone()),
                    None,
                )
            })?;
        let captured_at = Utc::now();

        let status = response.status();
        let raw = response.text().await.map_err(|error| {
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "read_response",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "supervisor proposal generation failed"
            );
            self.failure(
                SupervisorProposalFailureStage::Backend,
                format!("failed to read supervisor Responses API response body: {error}"),
                None,
                None,
                Some(prompt_render.clone()),
                None,
            )
        })?;
        let parsed_response = serde_json::from_str::<Value>(&raw);

        if !status.is_success() {
            let response_artifact = render_supervisor_response_artifact(
                "responses_api",
                self.config.supervisor.model.as_str(),
                parsed_response.as_ref().ok(),
                Some(raw.as_str()),
                None,
                captured_at,
            )
            .ok();
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "responses_api_status",
                status = %status,
                duration_ms = started_at.elapsed().as_millis() as u64,
                "supervisor proposal generation failed"
            );
            return Err(self.failure(
                SupervisorProposalFailureStage::Backend,
                format!(
                    "supervisor Responses API request failed with status {}: {}",
                    status, raw
                ),
                None,
                Some(raw),
                Some(prompt_render.clone()),
                response_artifact,
            ));
        }

        let value = parsed_response.map_err(|error| {
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "decode_response_json",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "supervisor proposal generation failed"
            );
            let response_artifact = render_supervisor_response_artifact(
                "responses_api",
                self.config.supervisor.model.as_str(),
                None,
                Some(raw.as_str()),
                None,
                captured_at,
            )
            .ok();
            self.failure(
                SupervisorProposalFailureStage::ResponseMalformed,
                format!("failed to decode supervisor Responses API response JSON: {error}"),
                None,
                Some(raw.clone()),
                Some(prompt_render.clone()),
                response_artifact,
            )
        })?;
        if let Some(error) = value.get("error") {
            if !error.is_null() {
                let response_artifact = render_supervisor_response_artifact(
                    "responses_api",
                    self.config.supervisor.model.as_str(),
                    Some(&value),
                    Some(raw.as_str()),
                    None,
                    captured_at,
                )
                .ok();
                warn!(
                    work_unit_id = %pack.primary_work_unit.id,
                    source_report_id = %pack.source_report.id,
                    stage = "response_error_payload",
                    response_id = value
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown"),
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "supervisor Responses API returned error payload"
                );
                return Err(self.failure(
                    SupervisorProposalFailureStage::Backend,
                    format!("supervisor Responses API returned an error payload: {error}"),
                    value
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    Some(raw.clone()),
                    Some(prompt_render.clone()),
                    response_artifact,
                ));
            }
        }

        let Some(output_text) = extract_output_text(&value) else {
            let response_artifact = render_supervisor_response_artifact(
                "responses_api",
                self.config.supervisor.model.as_str(),
                Some(&value),
                Some(raw.as_str()),
                None,
                captured_at,
            )
            .ok();
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "extract_output_text",
                response_id = value
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown"),
                duration_ms = started_at.elapsed().as_millis() as u64,
                "supervisor proposal generation failed"
            );
            return Err(self.failure(
                SupervisorProposalFailureStage::ResponseMalformed,
                "supervisor Responses API response did not contain assistant output_text",
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                Some(raw),
                Some(prompt_render.clone()),
                response_artifact,
            ));
        };
        let response_artifact = render_supervisor_response_artifact(
            "responses_api",
            self.config.supervisor.model.as_str(),
            Some(&value),
            Some(raw.as_str()),
            Some(output_text.as_str()),
            captured_at,
        )
        .map_err(|error| {
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "render_response_artifact",
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "supervisor proposal generation failed"
            );
            self.failure(
                SupervisorProposalFailureStage::Backend,
                error.to_string(),
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                Some(output_text.clone()),
                Some(prompt_render.clone()),
                None,
            )
        })?;
        let proposal: SupervisorProposal = serde_json::from_str(&output_text).map_err(|error| {
            warn!(
                work_unit_id = %pack.primary_work_unit.id,
                source_report_id = %pack.source_report.id,
                stage = "decode_proposal_json",
                response_id = value
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown"),
                duration_ms = started_at.elapsed().as_millis() as u64,
                error = %error,
                "supervisor proposal generation failed"
            );
            self.failure(
                SupervisorProposalFailureStage::ProposalMalformed,
                format!("failed to decode supervisor proposal JSON: {error}"),
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                Some(output_text.clone()),
                Some(prompt_render.clone()),
                Some(response_artifact.clone()),
            )
        })?;
        let usage = value.get("usage").map(extract_usage);
        info!(
            work_unit_id = %pack.primary_work_unit.id,
            source_report_id = %pack.source_report.id,
            trigger_kind = ?pack.trigger.kind,
            backend_kind = "responses_api",
            model = value
                .get("model")
                .and_then(|value| value.as_str())
                .unwrap_or(self.config.supervisor.model.as_str()),
            response_id = value
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            decision_type = snake_label(proposal.proposed_decision.decision_type),
            requires_assignment = proposal.proposed_decision.requires_assignment,
            duration_ms = started_at.elapsed().as_millis() as u64,
            "supervisor proposal generated"
        );

        Ok(SupervisorReasonerResult {
            proposal,
            backend_kind: "responses_api".to_string(),
            model: value
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(self.config.supervisor.model.as_str())
                .to_string(),
            response_id: value
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            usage,
            output_text: Some(output_text),
            prompt_render,
            response_artifact,
        })
    }
}

#[derive(Serialize)]
struct SupervisorPromptFingerprint<'a> {
    render_spec: &'a SupervisorPromptRenderSpec,
    instructions_text: &'a str,
    user_content_text: &'a str,
    context_pack_text: &'a str,
}

#[derive(Serialize)]
struct SupervisorResponseArtifactFingerprint<'a> {
    backend_kind: &'a str,
    model: &'a str,
    response_id: &'a Option<String>,
    usage: &'a Option<SupervisorReasonerUsage>,
    output_items: &'a [SupervisorResponseOutputItem],
    extracted_output_text: &'a Option<String>,
    raw_response_body: &'a Option<String>,
}

pub fn render_supervisor_prompt(
    pack: &SupervisorContextPack,
    rendered_at: DateTime<Utc>,
) -> OrcasResult<SupervisorPromptRenderArtifact> {
    let context_pack_text = serde_json::to_string_pretty(pack)?;
    let instructions_text = "You are the Orcas supervisor reasoner. Orcas state in the provided packet is the only source of truth. Use the canonical workstream plan, current focus item, exploration policy, and recent alignment assessments when deciding what to do next. Choose exactly one allowed decision for the primary work unit. Never invent ids, hidden context, or extra work units. Do not silently change the canonical plan; structural changes must be proposed for operator approval. Every assignment must be tied to a plan item or a narrow special activity kind. If the decision is continue or redirect, return one bounded draft next assignment. Return JSON only, matching the requested schema.".to_string();
    let user_content_text = format!(
        "Return a supervisor proposal JSON object for this Orcas decision point.\nThe packet already contains the allowed decision set and the canonical workstream state.\n\nSupervisorContextPack:\n{context_pack_text}"
    );
    let render_spec = SupervisorPromptRenderSpec {
        template_version: SUPERVISOR_PROMPT_TEMPLATE_VERSION.to_string(),
        context_schema_version: pack.schema_version.clone(),
        proposal_schema_name: SUPERVISOR_PROPOSAL_SCHEMA_NAME.to_string(),
        proposal_schema_version: PROPOSAL_SCHEMA_VERSION.to_string(),
        response_format: SUPERVISOR_RESPONSE_FORMAT.to_string(),
        strict_schema: true,
        context_serialization: SUPERVISOR_CONTEXT_SERIALIZATION.to_string(),
        style: SUPERVISOR_PROMPT_STYLE.to_string(),
    };
    let prompt_hash = json_fingerprint(&SupervisorPromptFingerprint {
        render_spec: &render_spec,
        instructions_text: &instructions_text,
        user_content_text: &user_content_text,
        context_pack_text: &context_pack_text,
    })?;
    Ok(SupervisorPromptRenderArtifact {
        render_spec,
        instructions_text,
        user_content_text,
        context_pack_text,
        prompt_hash,
        request_body_hash: None,
        rendered_at,
    })
}

pub fn render_supervisor_response_artifact(
    backend_kind: &str,
    fallback_model: &str,
    parsed_response: Option<&Value>,
    raw_response_body: Option<&str>,
    extracted_output_text: Option<&str>,
    captured_at: DateTime<Utc>,
) -> OrcasResult<SupervisorResponseArtifact> {
    let model = parsed_response
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str)
        .unwrap_or(fallback_model)
        .to_string();
    let response_id = parsed_response
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let usage = parsed_response
        .and_then(|value| value.get("usage"))
        .map(extract_usage);
    let output_items = parsed_response
        .map(extract_response_output_items)
        .unwrap_or_default();
    let raw_response_body = raw_response_body.map(ToOwned::to_owned);
    let raw_response_body_hash = raw_response_body.as_deref().map(stable_fingerprint);
    let extracted_output_text = extracted_output_text.map(ToOwned::to_owned);
    let response_hash = json_fingerprint(&SupervisorResponseArtifactFingerprint {
        backend_kind,
        model: &model,
        response_id: &response_id,
        usage: &usage,
        output_items: &output_items,
        extracted_output_text: &extracted_output_text,
        raw_response_body: &raw_response_body,
    })?;

    Ok(SupervisorResponseArtifact {
        backend_kind: backend_kind.to_string(),
        model,
        response_id,
        usage,
        output_items,
        extracted_output_text,
        response_hash,
        raw_response_body,
        raw_response_body_hash,
        captured_at,
    })
}

pub fn build_context_pack(
    collaboration: &CollaborationState,
    work_unit_id: &str,
    source_report_id: Option<&str>,
    requested_by: String,
    note: Option<String>,
    trigger_kind: SupervisorProposalTriggerKind,
) -> OrcasResult<SupervisorContextPack> {
    let started_at = Instant::now();
    info!(
        work_unit_id = %work_unit_id,
        source_report_id = source_report_id.unwrap_or("latest"),
        trigger_kind = ?trigger_kind,
        "building supervisor context pack"
    );
    let generated_at = Utc::now();
    let limits = SupervisorPackLimits {
        max_related_work_units: 8,
        max_prior_reports: 3,
        max_prior_decisions: 3,
        max_artifacts: 0,
        max_raw_report_chars: 3_000,
    };
    let work_unit = collaboration
        .work_units
        .get(work_unit_id)
        .cloned()
        .ok_or_else(|| OrcasError::Protocol(format!("unknown work unit `{work_unit_id}`")))?;
    let source_report = resolve_source_report(collaboration, &work_unit, source_report_id)?;
    let current_assignment = resolve_current_assignment(collaboration, &work_unit, &source_report)?;
    let worker_session = collaboration
        .worker_sessions
        .get(&current_assignment.worker_session_id)
        .cloned()
        .ok_or_else(|| {
            OrcasError::Protocol(format!(
                "unknown worker session `{}`",
                current_assignment.worker_session_id
            ))
        })?;
    let workstream = collaboration
        .workstreams
        .get(&work_unit.workstream_id)
        .cloned()
        .ok_or_else(|| {
            OrcasError::Protocol(format!("unknown workstream `{}`", work_unit.workstream_id))
        })?;
    let workstream_plan = build_workstream_plan_context(collaboration, &workstream.id);
    let latest_decision = latest_decision_for_work_unit(collaboration, &work_unit.id);
    let decision_policy =
        build_decision_policy(collaboration, &work_unit, &source_report, &worker_session)?;
    let (raw_output_excerpt, raw_output_truncated) =
        truncate_text(&source_report.raw_output, limits.max_raw_report_chars);
    let (upstream_dependencies, downstream_dependents) =
        build_dependency_context(collaboration, &work_unit);
    let dependency_context = SupervisorDependencyContext {
        upstream_dependencies,
        downstream_dependents,
    };
    let (related_work_units, related_truncated) =
        build_related_work_units(collaboration, &work_unit, &limits);
    let (recent_primary_history, reports_truncated, decisions_truncated) =
        build_recent_primary_history(collaboration, &work_unit, &limits);

    let pack = SupervisorContextPack {
        schema_version: CONTEXT_SCHEMA_VERSION.to_string(),
        generated_at,
        trigger: SupervisorProposalTrigger {
            kind: trigger_kind,
            requested_at: generated_at,
            requested_by,
            source_report_id: source_report.id.clone(),
            note: note.clone(),
        },
        pack_limits: limits,
        truncation: SupervisorPackTruncation {
            related_work_units_truncated: related_truncated,
            prior_reports_truncated: reports_truncated,
            prior_decisions_truncated: decisions_truncated,
            artifacts_truncated: false,
            raw_report_truncated: raw_output_truncated,
        },
        state_anchor: SupervisorStateAnchor {
            workstream_id: workstream.id.clone(),
            primary_work_unit_id: work_unit.id.clone(),
            source_report_id: source_report.id.clone(),
            source_report_created_at: source_report.created_at,
            current_assignment_id: work_unit.current_assignment_id.clone(),
            primary_work_unit_updated_at: work_unit.updated_at,
            latest_decision_id: latest_decision.as_ref().map(|decision| decision.id.clone()),
            latest_decision_created_at: latest_decision
                .as_ref()
                .map(|decision| decision.created_at),
        },
        decision_policy,
        workstream: build_workstream_context(collaboration, &workstream),
        workstream_plan,
        primary_work_unit: SupervisorWorkUnitContext {
            id: work_unit.id.clone(),
            title: work_unit.title.clone(),
            task_statement: work_unit.task_statement.clone(),
            status: label(&work_unit.status)?,
            dependencies: work_unit.dependencies.clone(),
            current_assignment_id: work_unit.current_assignment_id.clone(),
            latest_report_id: work_unit.latest_report_id.clone(),
            acceptance_criteria: Vec::new(),
            stop_conditions: Vec::new(),
            result_summary: None,
        },
        source_report: SupervisorSourceReportContext {
            id: source_report.id.clone(),
            assignment_id: source_report.assignment_id.clone(),
            worker_id: source_report.worker_id.clone(),
            worker_session_id: Some(current_assignment.worker_session_id.clone()),
            submitted_at: source_report.created_at,
            disposition: source_report.disposition,
            summary: source_report.summary.clone(),
            findings: source_report.findings.clone(),
            blockers: source_report.blockers.clone(),
            questions: source_report.questions.clone(),
            recommended_next_actions: source_report.recommended_next_actions.clone(),
            confidence: source_report.confidence,
            parse_result: source_report.parse_result,
            needs_supervisor_review: source_report.needs_supervisor_review,
            raw_output_excerpt,
        },
        current_assignment: SupervisorAssignmentContext {
            id: current_assignment.id.clone(),
            status: label(&current_assignment.status)?,
            attempt_number: current_assignment.attempt_number,
            plan_id: current_assignment.plan_id.as_ref().map(ToString::to_string),
            plan_version: current_assignment.plan_version,
            plan_item_id: current_assignment
                .plan_item_id
                .as_ref()
                .map(ToString::to_string),
            execution_kind: current_assignment.execution_kind,
            alignment_rationale: current_assignment.alignment_rationale.clone(),
            worker_id: current_assignment.worker_id.clone(),
            worker_session_id: current_assignment.worker_session_id.clone(),
            instructions: current_assignment.instructions.clone(),
            created_at: current_assignment.created_at,
            updated_at: current_assignment.updated_at,
        },
        worker_session: SupervisorWorkerSessionContext {
            id: worker_session.id.clone(),
            worker_id: worker_session.worker_id.clone(),
            backend_type: worker_session.backend_type.clone(),
            thread_id: worker_session.thread_id.clone(),
            active_turn_id: worker_session.active_turn_id.clone(),
            runtime_status: label(&worker_session.runtime_status)?,
            attachability: label(&worker_session.attachability)?,
            updated_at: worker_session.updated_at,
        },
        dependency_context,
        related_work_units,
        recent_primary_history,
        relevant_artifacts: Vec::<SupervisorArtifactRef>::new(),
        operator_request: note.map(|summary| SupervisorOperatorRequest {
            summary,
            focus: None,
            constraints: Vec::new(),
        }),
    };
    debug!(
        work_unit_id = %pack.primary_work_unit.id,
        source_report_id = %pack.source_report.id,
        related_work_unit_count = pack.related_work_units.len(),
        recent_report_count = pack.recent_primary_history.prior_reports.len(),
        recent_decision_count = pack.recent_primary_history.prior_decisions.len(),
        raw_report_truncated = pack.truncation.raw_report_truncated,
        duration_ms = started_at.elapsed().as_millis() as u64,
        "supervisor context pack built"
    );
    Ok(pack)
}

pub fn validate_proposal(
    proposal: &SupervisorProposal,
    pack: &SupervisorContextPack,
    collaboration: &CollaborationState,
) -> OrcasResult<()> {
    let started_at = Instant::now();
    debug!(
        work_unit_id = %pack.primary_work_unit.id,
        source_report_id = %pack.source_report.id,
        decision_type = snake_label(proposal.proposed_decision.decision_type),
        stage = "validate_proposal",
        "validating supervisor proposal"
    );

    let fail = |stage: &'static str, error: OrcasError| -> OrcasResult<()> {
        warn!(
            work_unit_id = %pack.primary_work_unit.id,
            source_report_id = %pack.source_report.id,
            decision_type = snake_label(proposal.proposed_decision.decision_type),
            stage,
            duration_ms = started_at.elapsed().as_millis() as u64,
            error = %error,
            "supervisor proposal validation failed"
        );
        Err(error)
    };

    if proposal.schema_version != PROPOSAL_SCHEMA_VERSION {
        return fail(
            "schema_version",
            OrcasError::Protocol(format!(
                "proposal schema version `{}` did not match `{PROPOSAL_SCHEMA_VERSION}`",
                proposal.schema_version
            )),
        );
    }

    let decision = proposal.proposed_decision.decision_type;
    if !pack.decision_policy.allowed_decisions.contains(&decision) {
        return fail(
            "allowed_decisions",
            OrcasError::Protocol(format!(
                "proposal decision `{}` is not allowed for this decision point",
                label(&decision)?
            )),
        );
    }
    if proposal.proposed_decision.target_work_unit_id != pack.primary_work_unit.id {
        return fail(
            "target_work_unit",
            OrcasError::Protocol("proposal targeted a different work unit".to_string()),
        );
    }
    if proposal.proposed_decision.source_report_id != pack.source_report.id {
        return fail(
            "source_report",
            OrcasError::Protocol("proposal targeted a different source report".to_string()),
        );
    }

    let requires_assignment = decision_requires_assignment(decision);
    if proposal.proposed_decision.requires_assignment != requires_assignment {
        return fail(
            "requires_assignment",
            OrcasError::Protocol(
                "proposal requires_assignment did not match Orcas policy".to_string(),
            ),
        );
    }
    let expected_status = expected_work_unit_status(decision);
    if proposal.proposed_decision.expected_work_unit_status != expected_status {
        return fail(
            "expected_work_unit_status",
            OrcasError::Protocol(format!(
                "proposal expected work-unit status `{}` did not match `{expected_status}`",
                proposal.proposed_decision.expected_work_unit_status
            )),
        );
    }

    if let Some(plan_revision) = proposal.plan_revision_proposal.as_ref() {
        validate_plan_revision_proposal(plan_revision, pack)?;
    }
    if let Some(assessment) = proposal.plan_assessment.as_ref() {
        validate_plan_assessment(assessment, pack)?;
    }

    match (&proposal.draft_next_assignment, requires_assignment) {
        (Some(_), false) => {
            return fail(
                "draft_assignment_forbidden",
                OrcasError::Protocol(
                    "proposal included a draft assignment for a decision that forbids one"
                        .to_string(),
                ),
            );
        }
        (None, true) => {
            return fail(
                "draft_assignment_required",
                OrcasError::Protocol("proposal omitted the required draft assignment".to_string()),
            );
        }
        (None, false) => {}
        (Some(draft), true) => validate_draft_assignment(draft, decision, pack, collaboration)?,
    }

    debug!(
        work_unit_id = %pack.primary_work_unit.id,
        source_report_id = %pack.source_report.id,
        decision_type = snake_label(decision),
        requires_assignment,
        duration_ms = started_at.elapsed().as_millis() as u64,
        "supervisor proposal validated"
    );
    Ok(())
}

pub fn apply_edits(
    proposal: &SupervisorProposal,
    edits: &SupervisorProposalEdits,
) -> SupervisorProposal {
    let mut updated = proposal.clone();

    if let Some(decision_type) = edits.decision_type {
        updated.proposed_decision.decision_type = decision_type;
        updated.proposed_decision.requires_assignment = decision_requires_assignment(decision_type);
        updated.proposed_decision.expected_work_unit_status =
            expected_work_unit_status(decision_type).to_string();
        if let Some(draft) = updated.draft_next_assignment.as_mut() {
            draft.derived_from_decision_type = decision_type;
        }
    }
    if let Some(rationale) = edits.decision_rationale.as_ref() {
        updated.proposed_decision.rationale = rationale.clone();
    }

    if updated.proposed_decision.requires_assignment {
        if let Some(draft) = updated.draft_next_assignment.as_mut() {
            if let Some(preferred_worker_id) = edits.preferred_worker_id.as_ref() {
                draft.preferred_worker_id = Some(preferred_worker_id.clone());
            }
            if let Some(worker_kind) = edits.worker_kind.as_ref() {
                draft.worker_kind = Some(worker_kind.clone());
            }
            if let Some(objective) = edits.objective.as_ref() {
                draft.objective = objective.clone();
            }
            if !edits.instructions.is_empty() {
                draft.instructions = edits.instructions.clone();
            }
            if !edits.acceptance_criteria.is_empty() {
                draft.acceptance_criteria = edits.acceptance_criteria.clone();
            }
            if !edits.stop_conditions.is_empty() {
                draft.stop_conditions = edits.stop_conditions.clone();
            }
            if !edits.expected_report_fields.is_empty() {
                draft.expected_report_fields = edits.expected_report_fields.clone();
            }
        }
    } else {
        updated.draft_next_assignment = None;
    }

    updated
}

pub fn compile_assignment_instructions(draft: &DraftAssignment, source_report_id: &str) -> String {
    debug!(
        predecessor_assignment_id = %draft.predecessor_assignment_id,
        source_report_id,
        decision_type = snake_label(draft.derived_from_decision_type),
        instruction_count = draft.instructions.len(),
        acceptance_count = draft.acceptance_criteria.len(),
        stop_condition_count = draft.stop_conditions.len(),
        expected_report_field_count = draft.expected_report_fields.len(),
        "compiling assignment instructions from supervisor draft"
    );
    let mut lines = vec![
        format!("Objective: {}", draft.objective),
        format!(
            "Derived decision: {}",
            snake_label(draft.derived_from_decision_type)
        ),
        format!(
            "Predecessor assignment: {}",
            draft.predecessor_assignment_id
        ),
        format!("Source report: {source_report_id}"),
    ];

    if !draft.required_context_refs.is_empty() {
        lines.push(format!(
            "Required context refs: {}",
            draft.required_context_refs.join(", ")
        ));
    }
    if !draft.instructions.is_empty() {
        lines.push("Instructions:".to_string());
        for instruction in &draft.instructions {
            lines.push(format!("- {instruction}"));
        }
    }
    if !draft.acceptance_criteria.is_empty() {
        lines.push("Acceptance criteria:".to_string());
        for criterion in &draft.acceptance_criteria {
            lines.push(format!("- {criterion}"));
        }
    }
    if !draft.stop_conditions.is_empty() {
        lines.push("Stop conditions:".to_string());
        for condition in &draft.stop_conditions {
            lines.push(format!("- {condition}"));
        }
    }
    if !draft.expected_report_fields.is_empty() {
        lines.push(format!(
            "Expected report fields: {}",
            draft.expected_report_fields.join(", ")
        ));
    }
    lines.push(format!("Boundedness note: {}", draft.boundedness_note));

    lines.join("\n")
}

pub fn proposal_freshness_error(
    proposal: &SupervisorProposalRecord,
    collaboration: &CollaborationState,
) -> Option<String> {
    state_anchor_freshness_error(&proposal.context_pack.state_anchor, collaboration)
}

pub fn state_anchor_freshness_error(
    anchor: &SupervisorStateAnchor,
    collaboration: &CollaborationState,
) -> Option<String> {
    let work_unit = collaboration.work_units.get(&anchor.primary_work_unit_id)?;
    if work_unit.status != WorkUnitStatus::AwaitingDecision {
        return Some(format!(
            "work unit left awaiting_decision and is now `{}`",
            snake_label(work_unit.status)
        ));
    }
    if work_unit.latest_report_id.as_deref() != Some(anchor.source_report_id.as_str()) {
        return Some("a newer report exists for the work unit".to_string());
    }
    if work_unit.current_assignment_id != anchor.current_assignment_id {
        return Some("the current assignment changed".to_string());
    }
    if work_unit.updated_at != anchor.primary_work_unit_updated_at {
        return Some("the work unit timestamp changed".to_string());
    }

    let report = collaboration.reports.get(&anchor.source_report_id)?;
    if report.created_at != anchor.source_report_created_at {
        return Some("the source report timestamp changed".to_string());
    }

    let latest_decision =
        latest_decision_for_work_unit(collaboration, &anchor.primary_work_unit_id);
    let latest_decision_id = latest_decision
        .as_ref()
        .map(|decision| decision.id.as_str());
    if latest_decision_id != anchor.latest_decision_id.as_deref() {
        return Some("a later decision was recorded for the work unit".to_string());
    }
    let latest_decision_created_at = latest_decision.as_ref().map(|decision| decision.created_at);
    if latest_decision_created_at != anchor.latest_decision_created_at {
        return Some("the latest decision timestamp changed".to_string());
    }

    None
}

fn resolve_source_report(
    collaboration: &CollaborationState,
    work_unit: &WorkUnit,
    source_report_id: Option<&str>,
) -> OrcasResult<Report> {
    let report_id = source_report_id
        .map(ToOwned::to_owned)
        .or_else(|| work_unit.latest_report_id.clone())
        .ok_or_else(|| {
            OrcasError::Protocol(format!("work unit `{}` has no latest report", work_unit.id))
        })?;
    if work_unit.latest_report_id.as_deref() != Some(report_id.as_str()) {
        return Err(OrcasError::Protocol(
            "proposal generation requires the latest report for the work unit".to_string(),
        ));
    }
    collaboration
        .reports
        .get(&report_id)
        .cloned()
        .ok_or_else(|| OrcasError::Protocol(format!("unknown source report `{report_id}`")))
}

fn resolve_current_assignment(
    collaboration: &CollaborationState,
    work_unit: &WorkUnit,
    source_report: &Report,
) -> OrcasResult<Assignment> {
    let assignment_id = work_unit
        .current_assignment_id
        .clone()
        .unwrap_or_else(|| source_report.assignment_id.clone());
    collaboration
        .assignments
        .get(&assignment_id)
        .cloned()
        .ok_or_else(|| OrcasError::Protocol(format!("unknown assignment `{assignment_id}`")))
}

fn build_workstream_context(
    collaboration: &CollaborationState,
    workstream: &orcas_core::Workstream,
) -> SupervisorWorkstreamContext {
    let units = collaboration
        .work_units
        .values()
        .filter(|unit| unit.workstream_id == workstream.id)
        .collect::<Vec<_>>();
    let blocked_work_unit_count = units
        .iter()
        .filter(|unit| {
            matches!(
                unit.status,
                WorkUnitStatus::Blocked | WorkUnitStatus::NeedsHuman
            )
        })
        .count();
    let completed_work_unit_count = units
        .iter()
        .filter(|unit| unit.status == WorkUnitStatus::Completed)
        .count();
    let open_work_unit_count = units.len().saturating_sub(completed_work_unit_count);

    SupervisorWorkstreamContext {
        id: workstream.id.clone(),
        title: workstream.title.clone(),
        objective: workstream.objective.clone(),
        status: snake_label(workstream.status),
        priority: workstream.priority.clone(),
        success_criteria: Vec::new(),
        constraints: Vec::new(),
        summary: None,
        open_work_unit_count,
        blocked_work_unit_count,
        completed_work_unit_count,
    }
}

fn build_workstream_plan_context(
    collaboration: &CollaborationState,
    workstream_id: &str,
) -> Option<SupervisorWorkstreamPlanContext> {
    let active_plan = collaboration.planning.active_plan(workstream_id)?.clone();
    let recent_assessments = collaboration
        .planning
        .recent_assessments_for_workstream(workstream_id, 5);
    let pending_revision_proposals = collaboration
        .planning
        .pending_revision_proposals_for_workstream(workstream_id)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    Some(SupervisorWorkstreamPlanContext {
        active_plan,
        recent_assessments,
        pending_revision_proposals,
    })
}

fn build_decision_policy(
    _collaboration: &CollaborationState,
    work_unit: &WorkUnit,
    report: &Report,
    worker_session: &WorkerSession,
) -> OrcasResult<DecisionPolicy> {
    let supported_decisions = vec![
        DecisionType::Accept,
        DecisionType::Continue,
        DecisionType::Redirect,
        DecisionType::MarkComplete,
        DecisionType::EscalateToHuman,
    ];
    let mut allowed_decisions = Vec::new();
    let mut disallowed_decisions = Vec::new();
    let mut disallowed_reasons_by_decision = BTreeMap::new();

    let report_quality = report_quality(report);
    let runtime_severity = runtime_severity(worker_session);

    for decision in &supported_decisions {
        let allowed = decision_allowed(*decision, report, report_quality, runtime_severity);
        if allowed {
            allowed_decisions.push(*decision);
        } else {
            disallowed_decisions.push(*decision);
            disallowed_reasons_by_decision.insert(
                snake_label(*decision),
                disallowed_reason(*decision, report, report_quality, runtime_severity),
            );
        }
    }

    if !allowed_decisions.contains(&DecisionType::EscalateToHuman) {
        return Err(OrcasError::Protocol(format!(
            "work unit `{}` reached a decision point without a human escalation path",
            work_unit.id
        )));
    }

    Ok(DecisionPolicy {
        supported_decisions,
        allowed_decisions,
        disallowed_decisions,
        disallowed_reasons_by_decision,
        assignment_required_for: vec![DecisionType::Continue, DecisionType::Redirect],
        assignment_forbidden_for: vec![
            DecisionType::Accept,
            DecisionType::MarkComplete,
            DecisionType::EscalateToHuman,
        ],
        human_review_required: true,
    })
}

fn build_dependency_context(
    collaboration: &CollaborationState,
    work_unit: &WorkUnit,
) -> (
    Vec<SupervisorDependencyContextItem>,
    Vec<SupervisorDependencyContextItem>,
) {
    let upstream_dependencies = work_unit
        .dependencies
        .iter()
        .filter_map(|dependency_id| {
            let dependency = collaboration.work_units.get(dependency_id)?;
            Some(SupervisorDependencyContextItem {
                work_unit_id: dependency.id.clone(),
                title: dependency.title.clone(),
                status: snake_label(dependency.status),
                latest_report_id: dependency.latest_report_id.clone(),
                latest_decision_id: latest_decision_for_work_unit(collaboration, &dependency.id)
                    .map(|decision| decision.id.clone()),
                relation: "blocks_on".to_string(),
                blocking: dependency.status != WorkUnitStatus::Completed,
            })
        })
        .collect::<Vec<_>>();

    let downstream_dependents = collaboration
        .work_units
        .values()
        .filter(|candidate| candidate.dependencies.contains(&work_unit.id))
        .map(|dependent| SupervisorDependencyContextItem {
            work_unit_id: dependent.id.clone(),
            title: dependent.title.clone(),
            status: snake_label(dependent.status),
            latest_report_id: dependent.latest_report_id.clone(),
            latest_decision_id: latest_decision_for_work_unit(collaboration, &dependent.id)
                .map(|decision| decision.id.clone()),
            relation: "blocked_by_primary".to_string(),
            blocking: dependent.status == WorkUnitStatus::Blocked,
        })
        .collect::<Vec<_>>();

    (upstream_dependencies, downstream_dependents)
}

fn build_related_work_units(
    collaboration: &CollaborationState,
    work_unit: &WorkUnit,
    limits: &SupervisorPackLimits,
) -> (Vec<RelatedWorkUnitContext>, bool) {
    let excluded = work_unit
        .dependencies
        .iter()
        .cloned()
        .chain(std::iter::once(work_unit.id.clone()))
        .chain(
            collaboration
                .work_units
                .values()
                .filter(|candidate| candidate.dependencies.contains(&work_unit.id))
                .map(|candidate| candidate.id.clone()),
        )
        .collect::<BTreeSet<_>>();

    let mut related = collaboration
        .work_units
        .values()
        .filter(|candidate| {
            candidate.workstream_id == work_unit.workstream_id && !excluded.contains(&candidate.id)
        })
        .map(|candidate| RelatedWorkUnitContext {
            id: candidate.id.clone(),
            title: candidate.title.clone(),
            status: snake_label(candidate.status),
            latest_report_summary: candidate
                .latest_report_id
                .as_ref()
                .and_then(|report_id| collaboration.reports.get(report_id))
                .map(|report| report.summary.clone()),
            latest_decision_type: latest_decision_for_work_unit(collaboration, &candidate.id)
                .map(|decision| decision.decision_type),
            updated_at: candidate.updated_at,
        })
        .collect::<Vec<_>>();

    related.sort_by(|left, right| {
        related_priority(&left.status)
            .cmp(&related_priority(&right.status))
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.id.cmp(&right.id))
    });
    let truncated = related.len() > limits.max_related_work_units;
    related.truncate(limits.max_related_work_units);
    (related, truncated)
}

fn build_recent_primary_history(
    collaboration: &CollaborationState,
    work_unit: &WorkUnit,
    limits: &SupervisorPackLimits,
) -> (RecentPrimaryHistory, bool, bool) {
    let mut reports = collaboration
        .reports
        .values()
        .filter(|report| {
            report.work_unit_id == work_unit.id
                && Some(report.id.as_str()) != work_unit.latest_report_id.as_deref()
        })
        .map(|report| PriorReportContext {
            id: report.id.clone(),
            disposition: report.disposition,
            summary: report.summary.clone(),
            parse_result: report.parse_result,
            needs_supervisor_review: report.needs_supervisor_review,
        })
        .collect::<Vec<_>>();
    reports.sort_by(|left, right| right.id.cmp(&left.id));
    let reports_truncated = reports.len() > limits.max_prior_reports;
    reports.truncate(limits.max_prior_reports);

    let mut decisions = collaboration
        .decisions
        .values()
        .filter(|decision| decision.work_unit_id == work_unit.id)
        .map(|decision| PriorDecisionContext {
            id: decision.id.clone(),
            decision_type: decision.decision_type,
            rationale: decision.rationale.clone(),
            created_at: decision.created_at,
        })
        .collect::<Vec<_>>();
    decisions.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let decisions_truncated = decisions.len() > limits.max_prior_decisions;
    decisions.truncate(limits.max_prior_decisions);

    (
        RecentPrimaryHistory {
            prior_reports: reports,
            prior_decisions: decisions,
        },
        reports_truncated,
        decisions_truncated,
    )
}

fn validate_draft_assignment(
    draft: &DraftAssignment,
    decision: DecisionType,
    pack: &SupervisorContextPack,
    collaboration: &CollaborationState,
) -> OrcasResult<()> {
    if draft.target_work_unit_id != pack.primary_work_unit.id {
        return Err(OrcasError::Protocol(
            "draft assignment targeted a different work unit".to_string(),
        ));
    }
    if draft.predecessor_assignment_id != pack.current_assignment.id {
        return Err(OrcasError::Protocol(
            "draft assignment predecessor_assignment_id did not match the current assignment"
                .to_string(),
        ));
    }
    if draft.derived_from_decision_type != decision {
        return Err(OrcasError::Protocol(
            "draft assignment derived_from_decision_type did not match the proposal decision"
                .to_string(),
        ));
    }
    if draft.objective.trim().is_empty() {
        return Err(OrcasError::Protocol(
            "draft assignment objective was empty".to_string(),
        ));
    }
    if draft.instructions.is_empty() || draft.instructions.len() > 7 {
        return Err(OrcasError::Protocol(
            "draft assignment must include between 1 and 7 instructions".to_string(),
        ));
    }
    if draft.acceptance_criteria.is_empty() || draft.acceptance_criteria.len() > 3 {
        return Err(OrcasError::Protocol(
            "draft assignment must include between 1 and 3 acceptance criteria".to_string(),
        ));
    }
    if draft.stop_conditions.is_empty() || draft.stop_conditions.len() > 3 {
        return Err(OrcasError::Protocol(
            "draft assignment must include between 1 and 3 stop conditions".to_string(),
        ));
    }
    if draft.expected_report_fields.is_empty() {
        return Err(OrcasError::Protocol(
            "draft assignment must declare at least one expected report field".to_string(),
        ));
    }
    for field in &draft.expected_report_fields {
        if !EXPECTED_REPORT_FIELDS.contains(&field.as_str()) {
            return Err(OrcasError::Protocol(format!(
                "draft assignment used an unknown expected report field `{field}`"
            )));
        }
    }
    for context_ref in &draft.required_context_refs {
        if !context_ref_exists(collaboration, context_ref) {
            return Err(OrcasError::Protocol(format!(
                "draft assignment referenced an unknown context ref `{context_ref}`"
            )));
        }
    }
    if let Some(worker_id) = draft.preferred_worker_id.as_ref() {
        if !collaboration.workers.contains_key(worker_id) {
            return Err(OrcasError::Protocol(format!(
                "draft assignment referenced an unknown worker `{worker_id}`"
            )));
        }
    }
    if draft.boundedness_note.trim().is_empty() {
        return Err(OrcasError::Protocol(
            "draft assignment must explain its boundedness".to_string(),
        ));
    }

    Ok(())
}

fn validate_plan_revision_proposal(
    proposal: &orcas_core::planning::PlanRevisionProposal,
    pack: &SupervisorContextPack,
) -> OrcasResult<()> {
    let Some(plan_context) = pack.workstream_plan.as_ref() else {
        return Err(OrcasError::Protocol(
            "plan revision proposal included without an active plan context".to_string(),
        ));
    };
    if proposal.workstream_id != pack.workstream.id {
        return Err(OrcasError::Protocol(
            "plan revision proposal targeted a different workstream".to_string(),
        ));
    }
    if proposal.base_plan_id != plan_context.active_plan.plan_id {
        return Err(OrcasError::Protocol(
            "plan revision proposal targeted a different active plan".to_string(),
        ));
    }
    if proposal.base_plan_version != plan_context.active_plan.version {
        return Err(OrcasError::Protocol(
            "plan revision proposal targeted a stale plan version".to_string(),
        ));
    }
    if proposal.ops.is_empty() {
        return Err(OrcasError::Protocol(
            "plan revision proposal must include at least one operation".to_string(),
        ));
    }
    if proposal.rationale.trim().is_empty() {
        return Err(OrcasError::Protocol(
            "plan revision proposal rationale was empty".to_string(),
        ));
    }
    if proposal.expected_benefit.trim().is_empty() || proposal.urgency.trim().is_empty() {
        return Err(OrcasError::Protocol(
            "plan revision proposal must include urgency and expected benefit".to_string(),
        ));
    }
    orcas_core::planning::validate_plan_revision_ops(&plan_context.active_plan, &proposal.ops)?;
    Ok(())
}

fn validate_plan_assessment(
    assessment: &orcas_core::planning::PlanAssessment,
    pack: &SupervisorContextPack,
) -> OrcasResult<()> {
    let Some(plan_context) = pack.workstream_plan.as_ref() else {
        return Err(OrcasError::Protocol(
            "plan assessment included without an active plan context".to_string(),
        ));
    };
    if assessment.workstream_id != pack.workstream.id {
        return Err(OrcasError::Protocol(
            "plan assessment targeted a different workstream".to_string(),
        ));
    }
    if assessment.plan_id != plan_context.active_plan.plan_id {
        return Err(OrcasError::Protocol(
            "plan assessment targeted a different active plan".to_string(),
        ));
    }
    if assessment.plan_version != plan_context.active_plan.version {
        return Err(OrcasError::Protocol(
            "plan assessment targeted a stale plan version".to_string(),
        ));
    }
    if assessment.progress_summary.trim().is_empty()
        || assessment.recommended_next_action.trim().is_empty()
    {
        return Err(OrcasError::Protocol(
            "plan assessment must include progress_summary and recommended_next_action".to_string(),
        ));
    }
    Ok(())
}

fn decision_requires_assignment(decision: DecisionType) -> bool {
    matches!(decision, DecisionType::Continue | DecisionType::Redirect)
}

fn expected_work_unit_status(decision: DecisionType) -> &'static str {
    match decision {
        DecisionType::Accept => "accepted",
        DecisionType::Continue | DecisionType::Redirect => "ready",
        DecisionType::MarkComplete => "completed",
        DecisionType::EscalateToHuman => "needs_human",
    }
}

fn report_quality(report: &Report) -> &'static str {
    match report.parse_result {
        ReportParseResult::Parsed if !report.needs_supervisor_review => "clean",
        ReportParseResult::Invalid => "invalid",
        _ => "ambiguous",
    }
}

fn runtime_severity(worker_session: &WorkerSession) -> &'static str {
    if matches!(
        worker_session.runtime_status,
        orcas_core::WorkerSessionRuntimeStatus::Interrupted
    ) {
        "interrupted"
    } else if matches!(
        worker_session.runtime_status,
        orcas_core::WorkerSessionRuntimeStatus::Lost
    ) || matches!(
        worker_session.attachability,
        orcas_core::WorkerSessionAttachability::Unknown
    ) {
        "lost_or_unknown"
    } else {
        "clean_terminal"
    }
}

fn decision_allowed(
    decision: DecisionType,
    report: &Report,
    report_quality: &str,
    runtime_severity: &str,
) -> bool {
    if report_quality != "clean" || runtime_severity != "clean_terminal" {
        return matches!(
            decision,
            DecisionType::Continue | DecisionType::Redirect | DecisionType::EscalateToHuman
        );
    }

    match report.disposition {
        ReportDisposition::Completed => true,
        ReportDisposition::Partial => matches!(
            decision,
            DecisionType::Accept
                | DecisionType::Continue
                | DecisionType::Redirect
                | DecisionType::EscalateToHuman
        ),
        ReportDisposition::Blocked
        | ReportDisposition::Failed
        | ReportDisposition::Interrupted
        | ReportDisposition::Unknown => matches!(
            decision,
            DecisionType::Continue | DecisionType::Redirect | DecisionType::EscalateToHuman
        ),
    }
}

fn disallowed_reason(
    decision: DecisionType,
    report: &Report,
    report_quality: &str,
    runtime_severity: &str,
) -> String {
    if matches!(
        decision,
        DecisionType::Continue | DecisionType::Redirect | DecisionType::EscalateToHuman
    ) {
        return "this decision remains available for bounded follow-up or human review".to_string();
    }
    if report_quality == "invalid" {
        return "invalid report parsing forces review instead of completion".to_string();
    }
    if report_quality == "ambiguous" {
        return "ambiguous report parsing forces review instead of completion".to_string();
    }
    if runtime_severity == "interrupted" {
        return "interrupted execution is not sufficient evidence of successful completion"
            .to_string();
    }
    if runtime_severity == "lost_or_unknown" {
        return "runtime continuity cannot be proven honestly".to_string();
    }

    match report.disposition {
        ReportDisposition::Partial if decision == DecisionType::MarkComplete => {
            "partial work cannot be marked complete yet".to_string()
        }
        ReportDisposition::Blocked => {
            "blocked work cannot be accepted or marked complete".to_string()
        }
        ReportDisposition::Failed => {
            "failed work cannot be accepted or marked complete".to_string()
        }
        ReportDisposition::Interrupted => {
            "interrupted work cannot be accepted or marked complete".to_string()
        }
        ReportDisposition::Unknown => {
            "unknown report disposition cannot be accepted or marked complete".to_string()
        }
        _ => "this decision is not allowed in the current work-unit state".to_string(),
    }
}

fn latest_decision_for_work_unit(
    collaboration: &CollaborationState,
    work_unit_id: &str,
) -> Option<Decision> {
    collaboration
        .decisions
        .values()
        .filter(|decision| decision.work_unit_id == work_unit_id)
        .cloned()
        .max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        })
}

fn truncate_text(raw: &str, max_chars: usize) -> (String, bool) {
    if raw.chars().count() <= max_chars {
        return (raw.to_string(), false);
    }
    let truncated = raw.chars().take(max_chars).collect::<String>();
    (truncated, true)
}

fn related_priority(status: &str) -> usize {
    match status {
        "ready" | "running" | "awaiting_decision" | "accepted" => 0,
        "blocked" | "needs_human" => 1,
        "completed" => 2,
        _ => 3,
    }
}

fn context_ref_exists(collaboration: &CollaborationState, context_ref: &str) -> bool {
    collaboration.workstreams.contains_key(context_ref)
        || collaboration.work_units.contains_key(context_ref)
        || collaboration.assignments.contains_key(context_ref)
        || collaboration.reports.contains_key(context_ref)
        || collaboration.decisions.contains_key(context_ref)
}

fn extract_output_text(value: &Value) -> Option<String> {
    let output = value.get("output")?.as_array()?;
    let mut text = String::new();
    for item in output {
        if item.get("type")?.as_str()? != "message" {
            continue;
        }
        let content = item.get("content")?.as_array()?;
        for part in content {
            if part.get("type")?.as_str()? == "output_text" {
                text.push_str(part.get("text")?.as_str()?);
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

fn extract_response_output_items(value: &Value) -> Vec<SupervisorResponseOutputItem> {
    value
        .get("output")
        .and_then(Value::as_array)
        .map(|output| output.iter().map(normalize_response_output_item).collect())
        .unwrap_or_default()
}

fn normalize_response_output_item(value: &Value) -> SupervisorResponseOutputItem {
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .map(normalize_response_content_part)
                .collect()
        })
        .unwrap_or_default();
    SupervisorResponseOutputItem {
        item_type: value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        role: value
            .get("role")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        content,
    }
}

fn normalize_response_content_part(value: &Value) -> SupervisorResponseContentPart {
    SupervisorResponseContentPart {
        part_type: value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        text: value
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    }
}

fn extract_usage(value: &Value) -> SupervisorReasonerUsage {
    SupervisorReasonerUsage {
        input_tokens: value.get("input_tokens").and_then(Value::as_u64),
        output_tokens: value.get("output_tokens").and_then(Value::as_u64),
        total_tokens: value.get("total_tokens").and_then(Value::as_u64),
    }
}

fn label<T>(value: &T) -> OrcasResult<String>
where
    T: Serialize,
{
    serde_json::to_value(value)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| OrcasError::Protocol("failed to serialize protocol label".to_string()))
}

fn snake_label<T>(value: T) -> String
where
    T: Serialize,
{
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".to_string())
}

fn proposal_json_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version",
            "summary",
            "proposed_decision",
            "draft_next_assignment",
            "confidence",
            "warnings",
            "open_questions"
        ],
        "properties": {
            "schema_version": {
                "type": "string",
                "const": PROPOSAL_SCHEMA_VERSION
            },
            "summary": {
                "type": "object",
                "additionalProperties": false,
                "required": [
                    "headline",
                    "situation",
                    "recommended_action",
                    "key_evidence",
                    "risks",
                    "review_focus"
                ],
                "properties": {
                    "headline": { "type": "string" },
                    "situation": { "type": "string" },
                    "recommended_action": { "type": "string" },
                    "key_evidence": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "risks": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "review_focus": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            },
            "proposed_decision": {
                "type": "object",
                "additionalProperties": false,
                "required": [
                    "decision_type",
                    "target_work_unit_id",
                    "source_report_id",
                    "rationale",
                    "expected_work_unit_status",
                    "requires_assignment"
                ],
                "properties": {
                    "decision_type": {
                        "type": "string",
                        "enum": [
                            "accept",
                            "continue",
                            "redirect",
                            "mark_complete",
                            "escalate_to_human"
                        ]
                    },
                    "target_work_unit_id": { "type": "string" },
                    "source_report_id": { "type": "string" },
                    "rationale": { "type": "string" },
                    "expected_work_unit_status": {
                        "type": "string",
                        "enum": ["accepted", "ready", "completed", "needs_human"]
                    },
                    "requires_assignment": { "type": "boolean" }
                }
            },
            "draft_next_assignment": {
                "type": ["object", "null"],
                "additionalProperties": false,
                "required": [
                    "target_work_unit_id",
                    "predecessor_assignment_id",
                    "derived_from_decision_type",
                    "plan_id",
                    "plan_version",
                    "plan_item_id",
                    "execution_kind",
                    "alignment_rationale",
                    "preferred_worker_id",
                    "worker_kind",
                    "objective",
                    "instructions",
                    "acceptance_criteria",
                    "stop_conditions",
                    "required_context_refs",
                    "expected_report_fields",
                    "boundedness_note"
                ],
                "properties": {
                    "target_work_unit_id": { "type": "string" },
                    "predecessor_assignment_id": { "type": "string" },
                    "derived_from_decision_type": {
                        "type": "string",
                        "enum": ["continue", "redirect"]
                    },
                    "plan_id": { "type": ["string", "null"] },
                    "plan_version": { "type": ["integer", "null"] },
                    "plan_item_id": { "type": ["string", "null"] },
                    "execution_kind": {
                        "type": "string",
                        "enum": [
                            "direct_execution",
                            "plan_bootstrap",
                            "plan_review",
                            "blocker_investigation",
                            "closure_synthesis"
                        ]
                    },
                    "alignment_rationale": { "type": ["string", "null"] },
                    "preferred_worker_id": { "type": ["string", "null"] },
                    "worker_kind": { "type": ["string", "null"] },
                    "objective": { "type": "string" },
                    "instructions": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "acceptance_criteria": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "stop_conditions": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "required_context_refs": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "expected_report_fields": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": EXPECTED_REPORT_FIELDS
                        }
                    },
                    "boundedness_note": { "type": "string" }
                }
            },
            "plan_assessment": {
                "type": ["object", "null"],
                "additionalProperties": false,
                "required": [
                    "assessment_id",
                    "workstream_id",
                    "plan_id",
                    "plan_version",
                    "assignment_id",
                    "plan_item_id",
                    "alignment_status",
                    "progress_summary",
                    "drift_risk",
                    "blocker_summary",
                    "recommended_next_action",
                    "proposed_revision_needed",
                    "execution_kind",
                    "created_at",
                    "created_by"
                ],
                "properties": {
                    "assessment_id": { "type": "string" },
                    "workstream_id": { "type": "string" },
                    "plan_id": { "type": "string" },
                    "plan_version": { "type": "integer" },
                    "assignment_id": { "type": ["string", "null"] },
                    "plan_item_id": { "type": ["string", "null"] },
                    "alignment_status": {
                        "type": "string",
                        "enum": [
                            "on_track",
                            "slight_drift",
                            "off_track",
                            "blocked",
                            "complete"
                        ]
                    },
                    "progress_summary": { "type": "string" },
                    "drift_risk": {
                        "type": "string",
                        "enum": ["low", "medium", "high"]
                    },
                    "blocker_summary": { "type": ["string", "null"] },
                    "recommended_next_action": { "type": "string" },
                    "proposed_revision_needed": { "type": "boolean" },
                    "execution_kind": {
                        "type": "string",
                        "enum": [
                            "direct_execution",
                            "plan_bootstrap",
                            "plan_review",
                            "blocker_investigation",
                            "closure_synthesis"
                        ]
                    },
                    "created_at": { "type": "string" },
                    "created_by": { "type": "string" }
                }
            },
            "plan_revision_proposal": {
                "type": ["object", "null"],
                "additionalProperties": false,
                "required": [
                    "proposal_id",
                    "workstream_id",
                    "base_plan_id",
                    "base_plan_version",
                    "rationale",
                    "urgency",
                    "expected_benefit",
                    "tradeoffs",
                    "ops",
                    "status",
                    "created_at",
                    "created_by",
                    "reviewed_at",
                    "reviewed_by",
                    "review_note",
                    "apply_started_at",
                    "apply_finished_at",
                    "apply_error",
                    "recovery",
                    "applied_plan_id",
                    "applied_plan_version",
                    "source_supervisor_proposal_id"
                ],
                "properties": {
                    "proposal_id": { "type": "string" },
                    "workstream_id": { "type": "string" },
                    "base_plan_id": { "type": "string" },
                    "base_plan_version": { "type": "integer" },
                    "rationale": { "type": "string" },
                    "urgency": { "type": "string" },
                    "expected_benefit": { "type": "string" },
                    "tradeoffs": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "ops": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false
                        }
                    },
                    "status": {
                        "type": "string",
                        "enum": [
                            "pending",
                            "approved",
                            "rejected",
                            "applied",
                            "superseded"
                        ]
                    },
                    "created_at": { "type": ["string", "null"] },
                    "created_by": { "type": ["string", "null"] }
                }
            },
            "confidence": {
                "type": "string",
                "enum": ["low", "medium", "high"]
            },
            "warnings": {
                "type": "array",
                "items": { "type": "string" }
            },
            "open_questions": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use orcas_core::planning::PlanExecutionKind;
    use orcas_core::supervisor::{
        DecisionPolicy, DraftAssignment, ProposedDecision, SupervisorAssignmentContext,
        SupervisorContextPack, SupervisorPackLimits, SupervisorPackTruncation, SupervisorProposal,
        SupervisorProposalEdits, SupervisorProposalTrigger, SupervisorProposalTriggerKind,
        SupervisorSourceReportContext, SupervisorStateAnchor, SupervisorSummary,
        SupervisorWorkUnitContext, SupervisorWorkerSessionContext, SupervisorWorkstreamContext,
    };
    use orcas_core::{
        Assignment, CollaborationState, Decision, DecisionType, Report, ReportConfidence,
        ReportDisposition, ReportParseResult, WorkUnit, WorkUnitStatus, Worker, WorkerSession,
        WorkerSessionAttachability, WorkerSessionRuntimeStatus, Workstream, WorkstreamStatus,
    };

    use super::{
        PROPOSAL_SCHEMA_VERSION, SUPERVISOR_PROMPT_TEMPLATE_VERSION, apply_edits,
        build_decision_policy, compile_assignment_instructions, decision_requires_assignment,
        expected_work_unit_status, render_supervisor_prompt, render_supervisor_response_artifact,
        state_anchor_freshness_error, validate_proposal,
    };

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 4, 5, 6, 7, 8)
            .single()
            .expect("valid timestamp")
    }

    fn sample_workstream() -> Workstream {
        Workstream {
            id: "ws-1".to_string(),
            title: "Workstream".to_string(),
            objective: "Complete bounded supervisor validation.".to_string(),
            status: WorkstreamStatus::Active,
            priority: "high".to_string(),
            created_at: fixed_now(),
            updated_at: fixed_now(),
        }
    }

    fn sample_work_unit() -> WorkUnit {
        WorkUnit {
            id: "wu-1".to_string(),
            workstream_id: "ws-1".to_string(),
            title: "Primary work unit".to_string(),
            task_statement: "Validate one proposal cleanly.".to_string(),
            status: WorkUnitStatus::AwaitingDecision,
            dependencies: Vec::new(),
            latest_report_id: Some("report-1".to_string()),
            current_assignment_id: Some("assignment-1".to_string()),
            created_at: fixed_now(),
            updated_at: fixed_now(),
        }
    }

    fn sample_assignment() -> Assignment {
        Assignment {
            id: "assignment-1".to_string(),
            work_unit_id: "wu-1".to_string(),
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
            worker_id: "worker-1".to_string(),
            worker_session_id: "session-1".to_string(),
            instructions: "Stay inside the bounded task.".to_string(),
            communication_seed: None,
            status: orcas_core::AssignmentStatus::AwaitingDecision,
            attempt_number: 1,
            created_at: fixed_now(),
            updated_at: fixed_now(),
        }
    }

    fn sample_worker() -> Worker {
        Worker {
            id: "worker-1".to_string(),
            kind: "codex".to_string(),
            status: Default::default(),
            current_assignment_id: Some("assignment-1".to_string()),
        }
    }

    fn sample_worker_session() -> WorkerSession {
        WorkerSession {
            id: "session-1".to_string(),
            worker_id: "worker-1".to_string(),
            backend_type: "codex".to_string(),
            thread_id: Some("thread-1".to_string()),
            tracked_thread_id: None,
            active_turn_id: None,
            runtime_status: WorkerSessionRuntimeStatus::Completed,
            attachability: WorkerSessionAttachability::Attachable,
            updated_at: fixed_now(),
        }
    }

    fn sample_report() -> Report {
        Report {
            id: "report-1".to_string(),
            work_unit_id: "wu-1".to_string(),
            assignment_id: "assignment-1".to_string(),
            worker_id: "worker-1".to_string(),
            disposition: ReportDisposition::Completed,
            summary: "Bounded work completed cleanly.".to_string(),
            findings: vec!["Parser contract tightened.".to_string()],
            blockers: Vec::new(),
            questions: Vec::new(),
            recommended_next_actions: Vec::new(),
            confidence: ReportConfidence::High,
            raw_output: "raw output".to_string(),
            parse_result: ReportParseResult::Parsed,
            needs_supervisor_review: false,
            created_at: fixed_now(),
        }
    }

    fn sample_collaboration() -> CollaborationState {
        let mut collaboration = CollaborationState::default();
        collaboration
            .workstreams
            .insert("ws-1".to_string(), sample_workstream());
        collaboration
            .work_units
            .insert("wu-1".to_string(), sample_work_unit());
        collaboration
            .assignments
            .insert("assignment-1".to_string(), sample_assignment());
        collaboration
            .workers
            .insert("worker-1".to_string(), sample_worker());
        collaboration
            .worker_sessions
            .insert("session-1".to_string(), sample_worker_session());
        collaboration
            .reports
            .insert("report-1".to_string(), sample_report());
        collaboration
    }

    fn sample_decision_policy(allowed_decisions: Vec<DecisionType>) -> DecisionPolicy {
        let supported_decisions = vec![
            DecisionType::Accept,
            DecisionType::Continue,
            DecisionType::Redirect,
            DecisionType::MarkComplete,
            DecisionType::EscalateToHuman,
        ];
        let disallowed_decisions = supported_decisions
            .iter()
            .copied()
            .filter(|decision| !allowed_decisions.contains(decision))
            .collect::<Vec<_>>();
        DecisionPolicy {
            supported_decisions,
            allowed_decisions,
            disallowed_decisions,
            disallowed_reasons_by_decision: std::collections::BTreeMap::new(),
            assignment_required_for: vec![DecisionType::Continue, DecisionType::Redirect],
            assignment_forbidden_for: vec![
                DecisionType::Accept,
                DecisionType::MarkComplete,
                DecisionType::EscalateToHuman,
            ],
            human_review_required: true,
        }
    }

    fn sample_pack(allowed_decisions: Vec<DecisionType>) -> SupervisorContextPack {
        SupervisorContextPack {
            schema_version: "supervisor_context_pack.v1".to_string(),
            generated_at: fixed_now(),
            trigger: SupervisorProposalTrigger {
                kind: SupervisorProposalTriggerKind::HumanRequested,
                requested_at: fixed_now(),
                requested_by: "operator".to_string(),
                source_report_id: "report-1".to_string(),
                note: Some("review this".to_string()),
            },
            pack_limits: SupervisorPackLimits {
                max_related_work_units: 8,
                max_prior_reports: 3,
                max_prior_decisions: 3,
                max_artifacts: 0,
                max_raw_report_chars: 3000,
            },
            truncation: SupervisorPackTruncation::default(),
            state_anchor: SupervisorStateAnchor {
                workstream_id: "ws-1".to_string(),
                primary_work_unit_id: "wu-1".to_string(),
                source_report_id: "report-1".to_string(),
                source_report_created_at: fixed_now(),
                current_assignment_id: Some("assignment-1".to_string()),
                primary_work_unit_updated_at: fixed_now(),
                latest_decision_id: None,
                latest_decision_created_at: None,
            },
            decision_policy: sample_decision_policy(allowed_decisions),
            workstream_plan: None,
            workstream: SupervisorWorkstreamContext {
                id: "ws-1".to_string(),
                title: "Workstream".to_string(),
                objective: "Complete bounded supervisor validation.".to_string(),
                status: "active".to_string(),
                priority: "high".to_string(),
                success_criteria: Vec::new(),
                constraints: Vec::new(),
                summary: None,
                open_work_unit_count: 1,
                blocked_work_unit_count: 0,
                completed_work_unit_count: 0,
            },
            primary_work_unit: SupervisorWorkUnitContext {
                id: "wu-1".to_string(),
                title: "Primary work unit".to_string(),
                task_statement: "Validate one proposal cleanly.".to_string(),
                status: "awaiting_decision".to_string(),
                dependencies: Vec::new(),
                current_assignment_id: Some("assignment-1".to_string()),
                latest_report_id: Some("report-1".to_string()),
                acceptance_criteria: Vec::new(),
                stop_conditions: Vec::new(),
                result_summary: None,
            },
            source_report: SupervisorSourceReportContext {
                id: "report-1".to_string(),
                assignment_id: "assignment-1".to_string(),
                worker_id: "worker-1".to_string(),
                worker_session_id: Some("session-1".to_string()),
                submitted_at: fixed_now(),
                disposition: ReportDisposition::Completed,
                summary: "Bounded work completed cleanly.".to_string(),
                findings: vec!["Parser contract tightened.".to_string()],
                blockers: Vec::new(),
                questions: Vec::new(),
                recommended_next_actions: Vec::new(),
                confidence: ReportConfidence::High,
                parse_result: ReportParseResult::Parsed,
                needs_supervisor_review: false,
                raw_output_excerpt: "raw output".to_string(),
            },
            current_assignment: SupervisorAssignmentContext {
                id: "assignment-1".to_string(),
                status: "awaiting_decision".to_string(),
                attempt_number: 1,
                plan_id: None,
                plan_version: None,
                plan_item_id: None,
                execution_kind: PlanExecutionKind::DirectExecution,
                alignment_rationale: None,
                worker_id: "worker-1".to_string(),
                worker_session_id: "session-1".to_string(),
                instructions: "Stay inside the bounded task.".to_string(),
                created_at: fixed_now(),
                updated_at: fixed_now(),
            },
            worker_session: SupervisorWorkerSessionContext {
                id: "session-1".to_string(),
                worker_id: "worker-1".to_string(),
                backend_type: "codex".to_string(),
                thread_id: Some("thread-1".to_string()),
                active_turn_id: None,
                runtime_status: "completed".to_string(),
                attachability: "attachable".to_string(),
                updated_at: fixed_now(),
            },
            dependency_context: Default::default(),
            related_work_units: Vec::new(),
            recent_primary_history: Default::default(),
            relevant_artifacts: Vec::new(),
            operator_request: None,
        }
    }

    fn sample_draft(decision: DecisionType) -> DraftAssignment {
        DraftAssignment {
            target_work_unit_id: "wu-1".to_string(),
            predecessor_assignment_id: "assignment-1".to_string(),
            derived_from_decision_type: decision,
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
            preferred_worker_id: Some("worker-1".to_string()),
            worker_kind: Some("codex".to_string()),
            objective: "Follow up on the bounded task.".to_string(),
            instructions: vec!["Inspect the bounded failure and fix it.".to_string()],
            acceptance_criteria: vec!["Keep the change bounded.".to_string()],
            stop_conditions: vec!["Stop if more scope is required.".to_string()],
            required_context_refs: vec!["ws-1".to_string(), "report-1".to_string()],
            expected_report_fields: vec!["summary".to_string(), "findings".to_string()],
            boundedness_note: "Do not broaden beyond the follow-up work.".to_string(),
        }
    }

    #[test]
    fn supervisor_prompt_render_is_deterministic_for_same_pack() {
        let pack = sample_pack(vec![DecisionType::Continue, DecisionType::Accept]);
        let first = render_supervisor_prompt(&pack, fixed_now()).expect("first render");
        let second = render_supervisor_prompt(&pack, fixed_now()).expect("second render");

        assert_eq!(first, second);
        assert_eq!(
            first.render_spec.template_version,
            SUPERVISOR_PROMPT_TEMPLATE_VERSION
        );
        assert_eq!(
            first.render_spec.context_schema_version,
            pack.schema_version
        );
        assert_eq!(
            first.render_spec.proposal_schema_version,
            PROPOSAL_SCHEMA_VERSION
        );
        assert_eq!(first.rendered_at, fixed_now());
        assert!(first.request_body_hash.is_none());
        assert!(
            first
                .instructions_text
                .contains("You are the Orcas supervisor reasoner.")
        );
        assert!(
            first
                .user_content_text
                .contains("Return a supervisor proposal JSON object")
        );
        assert!(first.user_content_text.contains(&first.context_pack_text));
        assert!(first.context_pack_text.contains("\"schema_version\""));
        assert!(!first.prompt_hash.is_empty());
    }

    #[test]
    fn supervisor_response_artifact_is_deterministic_for_same_response() {
        let raw = serde_json::json!({
            "id": "resp-1",
            "model": "test-supervisor",
            "usage": {
                "input_tokens": 12,
                "output_tokens": 34,
                "total_tokens": 46
            },
            "output": [{
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "{\"schema_version\":\"supervisor_proposal.v1\"}"
                }]
            }]
        });
        let raw_text = serde_json::to_string(&raw).expect("serialize response");
        let first = render_supervisor_response_artifact(
            "responses_api",
            "test-supervisor",
            Some(&raw),
            Some(raw_text.as_str()),
            Some("{\"schema_version\":\"supervisor_proposal.v1\"}"),
            fixed_now(),
        )
        .expect("first response artifact");
        let second = render_supervisor_response_artifact(
            "responses_api",
            "test-supervisor",
            Some(&raw),
            Some(raw_text.as_str()),
            Some("{\"schema_version\":\"supervisor_proposal.v1\"}"),
            fixed_now(),
        )
        .expect("second response artifact");

        assert_eq!(first, second);
        assert_eq!(first.response_id.as_deref(), Some("resp-1"));
        assert_eq!(
            first.extracted_output_text.as_deref(),
            Some("{\"schema_version\":\"supervisor_proposal.v1\"}")
        );
        assert_eq!(first.output_items.len(), 1);
        assert_eq!(first.output_items[0].item_type, "message");
        assert_eq!(first.output_items[0].content[0].part_type, "output_text");
        assert!(first.raw_response_body.is_some());
        assert!(first.raw_response_body_hash.is_some());
        assert!(!first.response_hash.is_empty());
    }

    fn sample_proposal(decision: DecisionType) -> SupervisorProposal {
        SupervisorProposal {
            schema_version: PROPOSAL_SCHEMA_VERSION.to_string(),
            summary: SupervisorSummary {
                headline: "Bounded recommendation".to_string(),
                situation: "A bounded supervisor decision is required.".to_string(),
                recommended_action: "Proceed with the chosen action.".to_string(),
                key_evidence: vec!["clean report".to_string()],
                risks: Vec::new(),
                review_focus: Vec::new(),
            },
            proposed_decision: ProposedDecision {
                decision_type: decision,
                target_work_unit_id: "wu-1".to_string(),
                source_report_id: "report-1".to_string(),
                rationale: "The bounded evidence supports this action.".to_string(),
                expected_work_unit_status: expected_work_unit_status(decision).to_string(),
                requires_assignment: decision_requires_assignment(decision),
            },
            draft_next_assignment: if decision_requires_assignment(decision) {
                Some(sample_draft(decision))
            } else {
                None
            },
            confidence: ReportConfidence::High,
            plan_assessment: None,
            plan_revision_proposal: None,
            warnings: Vec::new(),
            open_questions: Vec::new(),
        }
    }

    #[test]
    fn build_decision_policy_allows_completion_decisions_for_clean_completed_report() {
        let collaboration = sample_collaboration();
        let work_unit = collaboration.work_units["wu-1"].clone();
        let report = collaboration.reports["report-1"].clone();
        let worker_session = collaboration.worker_sessions["session-1"].clone();

        let policy = build_decision_policy(&collaboration, &work_unit, &report, &worker_session)
            .expect("decision policy");

        assert!(policy.allowed_decisions.contains(&DecisionType::Accept));
        assert!(
            policy
                .allowed_decisions
                .contains(&DecisionType::MarkComplete)
        );
        assert!(
            policy
                .allowed_decisions
                .contains(&DecisionType::EscalateToHuman)
        );
        assert!(policy.disallowed_decisions.is_empty());
    }

    #[test]
    fn build_decision_policy_for_ambiguous_report_disallows_completion() {
        let collaboration = sample_collaboration();
        let work_unit = collaboration.work_units["wu-1"].clone();
        let mut report = collaboration.reports["report-1"].clone();
        report.needs_supervisor_review = true;
        let worker_session = collaboration.worker_sessions["session-1"].clone();

        let policy = build_decision_policy(&collaboration, &work_unit, &report, &worker_session)
            .expect("decision policy");

        assert!(!policy.allowed_decisions.contains(&DecisionType::Accept));
        assert!(
            !policy
                .allowed_decisions
                .contains(&DecisionType::MarkComplete)
        );
        assert!(policy.allowed_decisions.contains(&DecisionType::Continue));
        assert!(policy.allowed_decisions.contains(&DecisionType::Redirect));
        assert_eq!(
            policy.disallowed_reasons_by_decision["accept"],
            "ambiguous report parsing forces review instead of completion"
        );
    }

    #[test]
    fn validate_proposal_accepts_clean_continue_proposal() {
        let collaboration = sample_collaboration();
        let pack = sample_pack(vec![
            DecisionType::Continue,
            DecisionType::Redirect,
            DecisionType::EscalateToHuman,
        ]);
        let proposal = sample_proposal(DecisionType::Continue);

        validate_proposal(&proposal, &pack, &collaboration).expect("proposal should validate");
    }

    #[test]
    fn proposal_schema_includes_nullable_plan_assessment_and_revision_fields_in_required_list() {
        let schema = super::proposal_json_schema();
        let plan_assessment_required = schema["properties"]["plan_assessment"]["required"]
            .as_array()
            .expect("plan_assessment required");
        for field in ["assignment_id", "plan_item_id", "blocker_summary"] {
            assert!(
                plan_assessment_required
                    .iter()
                    .any(|value: &serde_json::Value| value.as_str() == Some(field)),
                "plan_assessment schema must require {field}"
            );
        }

        let plan_revision_required = schema["properties"]["plan_revision_proposal"]["required"]
            .as_array()
            .expect("plan_revision_proposal required");
        for field in [
            "status",
            "created_at",
            "created_by",
            "reviewed_at",
            "reviewed_by",
            "review_note",
            "apply_started_at",
            "apply_finished_at",
            "apply_error",
            "recovery",
            "applied_plan_id",
            "applied_plan_version",
            "source_supervisor_proposal_id",
        ] {
            assert!(
                plan_revision_required
                    .iter()
                    .any(|value: &serde_json::Value| value.as_str() == Some(field)),
                "plan_revision_proposal schema must require {field}"
            );
        }
    }

    #[test]
    fn validate_proposal_rejects_disallowed_decision_type() {
        let collaboration = sample_collaboration();
        let pack = sample_pack(vec![DecisionType::EscalateToHuman]);
        let proposal = sample_proposal(DecisionType::Accept);

        let error =
            validate_proposal(&proposal, &pack, &collaboration).expect_err("proposal should fail");
        assert!(
            error
                .to_string()
                .contains("proposal decision `accept` is not allowed")
        );
    }

    #[test]
    fn validate_proposal_rejects_unknown_context_ref_in_draft() {
        let collaboration = sample_collaboration();
        let pack = sample_pack(vec![
            DecisionType::Continue,
            DecisionType::Redirect,
            DecisionType::EscalateToHuman,
        ]);
        let mut proposal = sample_proposal(DecisionType::Continue);
        proposal
            .draft_next_assignment
            .as_mut()
            .expect("draft")
            .required_context_refs
            .push("missing-context".to_string());

        let error =
            validate_proposal(&proposal, &pack, &collaboration).expect_err("proposal should fail");
        assert!(
            error
                .to_string()
                .contains("draft assignment referenced an unknown context ref `missing-context`")
        );
    }

    #[test]
    fn apply_edits_updates_decision_type_and_clears_forbidden_draft() {
        let proposal = sample_proposal(DecisionType::Continue);
        let edits = SupervisorProposalEdits {
            decision_type: Some(DecisionType::Accept),
            decision_rationale: Some("Accept the bounded work.".to_string()),
            ..Default::default()
        };

        let updated = apply_edits(&proposal, &edits);

        assert_eq!(
            updated.proposed_decision.decision_type,
            DecisionType::Accept
        );
        assert!(!updated.proposed_decision.requires_assignment);
        assert_eq!(
            updated.proposed_decision.expected_work_unit_status,
            "accepted"
        );
        assert_eq!(
            updated.proposed_decision.rationale,
            "Accept the bounded work."
        );
        assert!(updated.draft_next_assignment.is_none());
    }

    #[test]
    fn apply_edits_updates_existing_draft_fields_without_touching_others() {
        let proposal = sample_proposal(DecisionType::Continue);
        let edits = SupervisorProposalEdits {
            preferred_worker_id: Some("worker-1".to_string()),
            worker_kind: Some("codex-plus".to_string()),
            objective: Some("Investigate the remaining bounded issue.".to_string()),
            instructions: vec!["Reproduce the issue narrowly.".to_string()],
            acceptance_criteria: vec!["Document the bounded outcome.".to_string()],
            stop_conditions: vec!["Stop if a broader refactor is needed.".to_string()],
            expected_report_fields: vec!["questions".to_string()],
            ..Default::default()
        };

        let updated = apply_edits(&proposal, &edits);
        let draft = updated.draft_next_assignment.expect("draft should remain");

        assert_eq!(draft.preferred_worker_id.as_deref(), Some("worker-1"));
        assert_eq!(draft.worker_kind.as_deref(), Some("codex-plus"));
        assert_eq!(draft.objective, "Investigate the remaining bounded issue.");
        assert_eq!(
            draft.instructions,
            vec!["Reproduce the issue narrowly.".to_string()]
        );
        assert_eq!(
            draft.acceptance_criteria,
            vec!["Document the bounded outcome.".to_string()]
        );
        assert_eq!(
            draft.stop_conditions,
            vec!["Stop if a broader refactor is needed.".to_string()]
        );
        assert_eq!(draft.expected_report_fields, vec!["questions".to_string()]);
        assert_eq!(draft.derived_from_decision_type, DecisionType::Continue);
    }

    #[test]
    fn compile_assignment_instructions_renders_optional_sections_only_when_present() {
        let draft = DraftAssignment {
            target_work_unit_id: "wu-1".to_string(),
            predecessor_assignment_id: "assignment-1".to_string(),
            derived_from_decision_type: DecisionType::Redirect,
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
            preferred_worker_id: None,
            worker_kind: None,
            objective: "Follow the bounded redirect.".to_string(),
            instructions: vec!["Inspect the alternative bounded path.".to_string()],
            acceptance_criteria: vec!["Stay within redirect scope.".to_string()],
            stop_conditions: vec!["Stop when supervisor review is needed.".to_string()],
            required_context_refs: vec!["report-1".to_string()],
            expected_report_fields: vec!["summary".to_string(), "questions".to_string()],
            boundedness_note: "Do not broaden beyond the redirected task.".to_string(),
        };

        let rendered = compile_assignment_instructions(&draft, "report-1");

        assert!(rendered.contains("Objective: Follow the bounded redirect."));
        assert!(rendered.contains("Derived decision: redirect"));
        assert!(rendered.contains("Predecessor assignment: assignment-1"));
        assert!(rendered.contains("Source report: report-1"));
        assert!(rendered.contains("Required context refs: report-1"));
        assert!(rendered.contains("Instructions:\n- Inspect the alternative bounded path."));
        assert!(rendered.contains("Acceptance criteria:\n- Stay within redirect scope."));
        assert!(rendered.contains("Stop conditions:\n- Stop when supervisor review is needed."));
        assert!(rendered.contains("Expected report fields: summary, questions"));
        assert!(rendered.contains("Boundedness note: Do not broaden beyond the redirected task."));
    }

    #[test]
    fn state_anchor_freshness_error_detects_newer_report_for_work_unit() {
        let mut collaboration = sample_collaboration();
        let now = fixed_now();
        collaboration.decisions.insert(
            "decision-1".to_string(),
            Decision {
                id: "decision-1".to_string(),
                work_unit_id: "wu-1".to_string(),
                report_id: Some("report-1".to_string()),
                decision_type: DecisionType::Continue,
                rationale: "Keep going".to_string(),
                created_at: now,
            },
        );

        let mut anchor = sample_pack(vec![DecisionType::EscalateToHuman]).state_anchor;
        anchor.latest_decision_id = Some("decision-1".to_string());
        anchor.latest_decision_created_at = Some(now);
        collaboration
            .work_units
            .get_mut("wu-1")
            .expect("work unit")
            .latest_report_id = Some("report-2".to_string());

        let error = state_anchor_freshness_error(&anchor, &collaboration);

        assert_eq!(
            error.as_deref(),
            Some("a newer report exists for the work unit")
        );
    }
}
