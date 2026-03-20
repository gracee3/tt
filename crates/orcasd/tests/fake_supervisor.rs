use axum::{Json, Router, routing::post};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use orcas_core::supervisor::{
    DraftAssignment, ProposedDecision, SupervisorContextPack, SupervisorProposal, SupervisorSummary,
};
use orcas_core::{DecisionType, ReportConfidence};

pub struct FakeSupervisorResponsesServer {
    pub base_url: String,
    task: JoinHandle<()>,
}

impl FakeSupervisorResponsesServer {
    pub async fn spawn() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fake supervisor Responses server");
        let base_url = format!(
            "http://127.0.0.1:{}",
            listener
                .local_addr()
                .expect("fake supervisor listener address")
                .port()
        );
        let app = Router::new().route("/responses", post(Self::handle_responses));
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake supervisor Responses server");
        });
        Self { base_url, task }
    }

    async fn handle_responses(Json(request): Json<serde_json::Value>) -> Json<serde_json::Value> {
        let pack = Self::extract_context_pack(&request);
        let proposal = Self::build_proposal(&pack);
        let proposal_text =
            serde_json::to_string(&proposal).expect("serialize fake supervisor proposal");
        Json(serde_json::json!({
            "id": "resp_fake_supervisor",
            "model": "fake-supervisor-model",
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": proposal_text,
                }]
            }],
            "input_tokens": 128,
            "output_tokens": 64,
            "total_tokens": 192
        }))
    }

    fn extract_context_pack(request: &serde_json::Value) -> SupervisorContextPack {
        let prompt = request["input"][0]["content"][0]["text"]
            .as_str()
            .expect("fake supervisor input text");
        let (_, pack_json) = prompt
            .split_once("SupervisorContextPack:\n")
            .expect("supervisor prompt should contain context pack");
        serde_json::from_str(pack_json.trim()).expect("decode fake supervisor context pack")
    }

    fn build_proposal(pack: &SupervisorContextPack) -> SupervisorProposal {
        let decision = Self::choose_decision(pack);
        let requires_assignment =
            matches!(decision, DecisionType::Continue | DecisionType::Redirect);
        SupervisorProposal {
            schema_version: "supervisor_proposal.v1".to_string(),
            summary: SupervisorSummary {
                headline: format!(
                    "Deterministic proposal for {}",
                    pack.primary_work_unit.title
                ),
                situation: format!(
                    "Operator review for work unit `{}` sourced from report `{}`.",
                    pack.primary_work_unit.id, pack.source_report.id
                ),
                recommended_action: format!(
                    "Apply {:?} for work unit `{}`.",
                    decision, pack.primary_work_unit.id
                ),
                key_evidence: vec![pack.source_report.summary.clone()],
                risks: vec!["Bounded fake supervisor response".to_string()],
                review_focus: vec!["Keep the workflow deterministic".to_string()],
            },
            proposed_decision: ProposedDecision {
                decision_type: decision,
                target_work_unit_id: pack.primary_work_unit.id.clone(),
                source_report_id: pack.source_report.id.clone(),
                rationale: format!(
                    "Deterministic fake supervisor proposal for assignment `{}`.",
                    pack.current_assignment.id
                ),
                expected_work_unit_status: Self::expected_work_unit_status(decision).to_string(),
                requires_assignment,
            },
            draft_next_assignment: requires_assignment.then(|| DraftAssignment {
                target_work_unit_id: pack.primary_work_unit.id.clone(),
                predecessor_assignment_id: pack.current_assignment.id.clone(),
                derived_from_decision_type: decision,
                preferred_worker_id: Some(pack.current_assignment.worker_id.clone()),
                worker_kind: Some("codex".to_string()),
                objective: format!(
                    "Follow up on report `{}` for work unit `{}`.",
                    pack.source_report.id, pack.primary_work_unit.id
                ),
                instructions: vec!["Inspect the bounded follow-up branch only.".to_string()],
                acceptance_criteria: vec![
                    "Confirm the next bounded action for this work unit.".to_string(),
                ],
                stop_conditions: vec![
                    "Stop if the follow-up would broaden beyond the current work unit.".to_string(),
                ],
                required_context_refs: Vec::new(),
                expected_report_fields: vec!["summary".to_string(), "findings".to_string()],
                boundedness_note:
                    "This fake proposal intentionally stays within one follow-up step.".to_string(),
            }),
            confidence: ReportConfidence::Medium,
            warnings: vec!["Generated by the fake supervisor Responses server".to_string()],
            open_questions: Vec::new(),
        }
    }

    fn choose_decision(pack: &SupervisorContextPack) -> DecisionType {
        for preferred in [
            DecisionType::Accept,
            DecisionType::Continue,
            DecisionType::Redirect,
            DecisionType::MarkComplete,
            DecisionType::EscalateToHuman,
        ] {
            if pack.decision_policy.allowed_decisions.contains(&preferred) {
                return preferred;
            }
        }
        panic!(
            "fake supervisor found no allowed decision for work unit {}",
            pack.primary_work_unit.id
        );
    }

    fn expected_work_unit_status(decision: DecisionType) -> &'static str {
        match decision {
            DecisionType::Accept => "accepted",
            DecisionType::Continue | DecisionType::Redirect => "ready",
            DecisionType::MarkComplete => "completed",
            DecisionType::EscalateToHuman => "needs_human",
        }
    }
}

impl Drop for FakeSupervisorResponsesServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
