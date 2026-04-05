use chrono::{DateTime, Utc};
use uuid::Uuid;

use tt_core::collaboration::{
    PlanningSession, PlanningSessionStatus, WorkUnit, WorkUnitStatus, Workstream,
};
use tt_core::planning::{self, PlanRevisionOp, PlanRevisionProposal, PlanRevisionProposalId};
use tt_core::{TTError, TTResult};

pub fn planning_session_thread_prompt(
    session: &PlanningSession,
    workstream: &Workstream,
    active_plan: Option<&planning::WorkstreamPlan>,
) -> String {
    let summary = &session.latest_structured_summary;
    let mut prompt = String::new();
    prompt.push_str("You are the TT planning delegate for a supervisor-owned planning session.\n");
    prompt.push_str("Stay strictly pre-execution.\n");
    prompt.push_str("Do not implement code, mutate canonical plan state, or create execution assignments unless the supervisor explicitly requests bounded research.\n\n");
    prompt.push_str(&format!("Planning session: {}\n", session.session_id));
    prompt.push_str(&format!(
        "Workstream: {} ({})\n",
        workstream.id, workstream.title
    ));
    prompt.push_str(&format!("Objective: {}\n", summary.objective));
    if let Some(plan) = active_plan {
        prompt.push_str(&format!(
            "Active canonical plan: {} v{}\n",
            plan.plan_id, plan.version
        ));
    }
    prompt.push_str(&format!("Research status: {:?}\n", summary.research_status));
    prompt.push_str(&format!("Ready for review: {}\n", summary.ready_for_review));
    if !summary.requirements.is_empty() {
        prompt.push_str("Requirements:\n");
        for requirement in &summary.requirements {
            prompt.push_str(&format!("- {requirement}\n"));
        }
    }
    if !summary.constraints.is_empty() {
        prompt.push_str("Constraints:\n");
        for constraint in &summary.constraints {
            prompt.push_str(&format!("- {constraint}\n"));
        }
    }
    if !summary.non_goals.is_empty() {
        prompt.push_str("Non-goals:\n");
        for non_goal in &summary.non_goals {
            prompt.push_str(&format!("- {non_goal}\n"));
        }
    }
    if !summary.open_questions.is_empty() {
        prompt.push_str("Open questions:\n");
        for question in &summary.open_questions {
            prompt.push_str(&format!("- {question}\n"));
        }
    }
    prompt.push_str("\nTyped control actions available to this planning session:\n");
    prompt.push_str(
        "- RequestSupervisorContext: ask the supervisor for more context or decisions.\n",
    );
    prompt.push_str("- RequestResearch: ask TT to run one bounded research assignment.\n");
    prompt.push_str("- SubmitPlanningSummary: publish a structured planning summary.\n");
    prompt.push_str("- MarkReadyForReview: indicate the plan is ready for approval handoff.\n");
    prompt.push_str(
        "- AbortPlanning: stop this planning session without changing canonical plan state.\n\n",
    );
    prompt.push_str(
        "When you need one of those actions, state it explicitly and keep the request bounded.\n",
    );
    prompt
}

pub fn build_planning_revision_proposal(
    session: &PlanningSession,
    active_plan: &planning::WorkstreamPlan,
    created_by: &str,
    now: DateTime<Utc>,
) -> TTResult<PlanRevisionProposal> {
    // This only stages a canonical plan revision proposal.
    // Actual plan mutation still flows through the existing plan revision
    // approval/apply path.
    let summary = &session.latest_structured_summary;
    let mut ops = Vec::new();
    if !summary.requirements.is_empty() {
        ops.push(PlanRevisionOp::UpdateSuccessCriteria {
            success_criteria: summary.requirements.clone(),
        });
    }
    let mut combined_constraints = summary.constraints.clone();
    combined_constraints.extend(summary.non_goals.clone());
    if !combined_constraints.is_empty() {
        ops.push(PlanRevisionOp::UpdateConstraints {
            constraints: combined_constraints,
        });
    }
    if ops.is_empty() {
        ops.push(PlanRevisionOp::UpdateSuccessCriteria {
            success_criteria: vec![summary.objective.clone()],
        });
    }
    let rationale = summary
        .draft_plan_summary
        .clone()
        .unwrap_or_else(|| summary.objective.clone());
    if rationale.trim().is_empty() {
        return Err(TTError::Protocol(
            "planning session approval requires a non-empty objective or draft plan summary"
                .to_string(),
        ));
    }
    Ok(PlanRevisionProposal {
        proposal_id: PlanRevisionProposalId::parse(format!(
            "planning-revision-{}",
            Uuid::now_v7().simple()
        ))?,
        workstream_id: session.workstream_id.clone(),
        base_plan_id: active_plan.plan_id.clone(),
        base_plan_version: active_plan.version,
        rationale,
        urgency: "medium".to_string(),
        expected_benefit: "hand off the planning session into canonical plan revision approval"
            .to_string(),
        tradeoffs: summary.non_goals.clone(),
        ops,
        status: planning::PlanRevisionProposalStatus::Pending,
        created_at: now,
        created_by: created_by.to_string(),
        reviewed_at: None,
        reviewed_by: None,
        review_note: None,
        apply_started_at: None,
        apply_finished_at: None,
        apply_error: None,
        recovery: planning::PlanRevisionRecoveryState::default(),
        applied_plan_id: None,
        applied_plan_version: None,
        source_supervisor_proposal_id: None,
    })
}

pub fn build_research_work_unit(
    workstream: &Workstream,
    session: &PlanningSession,
    research_work_unit_id: String,
    created_at: DateTime<Utc>,
) -> WorkUnit {
    WorkUnit {
        id: research_work_unit_id,
        workstream_id: workstream.id.clone(),
        title: format!("Planning research for {}", session.session_id),
        task_statement: session.latest_structured_summary.objective.clone(),
        status: WorkUnitStatus::Ready,
        dependencies: Vec::new(),
        latest_report_id: None,
        current_assignment_id: None,
        created_at,
        updated_at: created_at,
    }
}

pub fn planning_session_status_is_terminal(status: PlanningSessionStatus) -> bool {
    matches!(
        status,
        PlanningSessionStatus::Approved
            | PlanningSessionStatus::Rejected
            | PlanningSessionStatus::Superseded
            | PlanningSessionStatus::Aborted
    )
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{
        build_planning_revision_proposal, build_research_work_unit, planning_session_thread_prompt,
    };
    use tt_core::collaboration::{
        PlanningSession, PlanningSessionResearchStatus, PlanningSessionStatus,
        PlanningSessionStructuredSummary, Workstream, WorkstreamStatus,
    };
    use tt_core::planning::{PlanGoal, PlanGoalId, PlanId, PlanStatus, WorkstreamPlan};

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 20, 14, 15, 16)
            .single()
            .expect("valid timestamp")
    }

    fn sample_workstream() -> Workstream {
        Workstream {
            id: "ws-1".to_string(),
            title: "Core workstream".to_string(),
            objective: "Ship one bounded change.".to_string(),
            status: WorkstreamStatus::Active,
            priority: "high".to_string(),
            created_at: fixed_now(),
            updated_at: fixed_now(),
        }
    }

    fn sample_plan() -> WorkstreamPlan {
        WorkstreamPlan {
            plan_id: PlanId::parse("plan-1").expect("plan id"),
            workstream_id: "ws-1".to_string(),
            version: 1,
            status: PlanStatus::Active,
            title: "Core workstream".to_string(),
            overview: Some("Ship one bounded change.".to_string()),
            goals: vec![PlanGoal {
                goal_id: PlanGoalId::parse("goal-1").expect("goal id"),
                title: "Core workstream".to_string(),
                description: Some("Ship one bounded change.".to_string()),
                priority: "high".to_string(),
                status: Default::default(),
            }],
            plan_items: Vec::new(),
            success_criteria: Vec::new(),
            constraints: Vec::new(),
            exploration_policy: Default::default(),
            current_focus_item_id: None,
            created_at: fixed_now(),
            updated_at: fixed_now(),
            created_by: "creator".to_string(),
            updated_by: "creator".to_string(),
            superseded_by_plan_id: None,
            source_revision_proposal_id: None,
        }
    }

    fn sample_session() -> PlanningSession {
        PlanningSession {
            session_id: "ps-1".to_string(),
            workstream_id: "ws-1".to_string(),
            status: PlanningSessionStatus::Chatting,
            planning_thread_id: "thread-1".to_string(),
            base_plan_id: Some(PlanId::parse("plan-1").expect("plan id")),
            base_plan_version: Some(1),
            research_assignment_id: None,
            research_report_id: None,
            draft_revision_proposal_id: None,
            approved_plan_id: None,
            approved_plan_version: None,
            latest_structured_summary: PlanningSessionStructuredSummary {
                objective: "Ship one bounded change.".to_string(),
                requirements: vec!["Capture the result cleanly.".to_string()],
                constraints: vec!["Keep scope narrow.".to_string()],
                non_goals: vec!["No broad refactor.".to_string()],
                open_questions: vec!["Do we need research?".to_string()],
                research_status: PlanningSessionResearchStatus::NotRequested,
                draft_plan_summary: Some("Narrow v1 planning pass.".to_string()),
                ready_for_review: false,
            },
            created_at: fixed_now(),
            created_by: "supervisor".to_string(),
            updated_at: fixed_now(),
            updated_by: "supervisor".to_string(),
            request_note: None,
            reviewed_at: None,
            reviewed_by: None,
            review_note: None,
            superseded_by_session_id: None,
        }
    }

    #[test]
    fn prompt_mentions_typed_actions_and_objective() {
        let prompt = planning_session_thread_prompt(
            &sample_session(),
            &sample_workstream(),
            Some(&sample_plan()),
        );
        assert!(prompt.contains("planning delegate"));
        assert!(prompt.contains("RequestResearch"));
        assert!(prompt.contains("Ship one bounded change."));
        assert!(prompt.contains("Research status"));
        assert!(prompt.contains("Ready for review"));
    }

    #[test]
    fn build_revision_proposal_uses_summary_content() {
        let proposal = build_planning_revision_proposal(
            &sample_session(),
            &sample_plan(),
            "supervisor",
            fixed_now(),
        )
        .expect("proposal");
        assert_eq!(proposal.workstream_id, "ws-1");
        assert_eq!(proposal.base_plan_version, 1);
        assert_eq!(proposal.created_by, "supervisor");
        assert!(!proposal.ops.is_empty());
    }

    #[test]
    fn build_research_work_unit_uses_session_objective() {
        let work_unit = build_research_work_unit(
            &sample_workstream(),
            &sample_session(),
            "wu-1".to_string(),
            fixed_now(),
        );
        assert_eq!(work_unit.workstream_id, "ws-1");
        assert_eq!(work_unit.title, "Planning research for ps-1");
    }
}
