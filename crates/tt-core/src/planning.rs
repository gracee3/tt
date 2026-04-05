use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::collaboration::{WorkUnit, WorkUnitStatus, Workstream};
use crate::{TTError, TTResult};

fn require_non_empty(value: impl Into<String>, field: &str) -> TTResult<String> {
    let value = value.into();
    if value.trim().is_empty() {
        Err(TTError::Protocol(format!("{field} cannot be empty")))
    } else {
        Ok(value)
    }
}

fn default_utc_now() -> DateTime<Utc> {
    Utc::now()
}

macro_rules! non_empty_string_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> TTResult<Self> {
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

non_empty_string_type!(PlanId);
non_empty_string_type!(PlanGoalId);
non_empty_string_type!(PlanItemId);
non_empty_string_type!(PlanAssessmentId);
non_empty_string_type!(PlanRevisionProposalId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExplorationMode {
    Strict,
    #[default]
    Balanced,
    Exploratory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationPolicy {
    #[serde(default)]
    pub mode: ExplorationMode,
    #[serde(default)]
    pub max_branch_depth: Option<u32>,
    #[serde(default)]
    pub allow_blocker_investigations: bool,
    #[serde(default)]
    pub allow_speculative_side_paths: bool,
    #[serde(default)]
    pub checkpoint_interval: Option<u32>,
    #[serde(default)]
    pub drift_alert_threshold: Option<String>,
}

impl Default for ExplorationPolicy {
    fn default() -> Self {
        Self {
            mode: ExplorationMode::Balanced,
            max_branch_depth: Some(1),
            allow_blocker_investigations: true,
            allow_speculative_side_paths: false,
            checkpoint_interval: Some(3),
            drift_alert_threshold: Some("medium".to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Draft,
    #[default]
    Active,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanGoalStatus {
    #[default]
    Pending,
    InProgress,
    Complete,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    #[default]
    Pending,
    InProgress,
    Blocked,
    Done,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AlignmentStatus {
    #[default]
    OnTrack,
    SlightDrift,
    OffTrack,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DriftRisk {
    #[default]
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanExecutionKind {
    #[default]
    DirectExecution,
    PlanBootstrap,
    PlanReview,
    BlockerInvestigation,
    ClosureSynthesis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanGoal {
    pub goal_id: PlanGoalId,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub status: PlanGoalStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    pub item_id: PlanItemId,
    pub goal_id: PlanGoalId,
    pub title: String,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub status: PlanItemStatus,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub dependency_item_ids: Vec<PlanItemId>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub linked_work_unit_id: Option<String>,
    #[serde(default)]
    pub linked_assignment_ids: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkstreamPlan {
    pub plan_id: PlanId,
    pub workstream_id: String,
    pub version: u64,
    #[serde(default)]
    pub status: PlanStatus,
    pub title: String,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub goals: Vec<PlanGoal>,
    #[serde(default)]
    pub plan_items: Vec<PlanItem>,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub exploration_policy: ExplorationPolicy,
    #[serde(default)]
    pub current_focus_item_id: Option<PlanItemId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: String,
    pub updated_by: String,
    #[serde(default)]
    pub superseded_by_plan_id: Option<PlanId>,
    #[serde(default)]
    pub source_revision_proposal_id: Option<PlanRevisionProposalId>,
}

impl WorkstreamPlan {
    #[must_use]
    pub fn active_focus_item(&self) -> Option<&PlanItem> {
        let focus_id = self.current_focus_item_id.as_ref()?;
        self.plan_items
            .iter()
            .find(|item| &item.item_id == focus_id)
    }

    pub fn validate(&self) -> TTResult<()> {
        require_non_empty(self.workstream_id.clone(), "workstream_id")?;
        require_non_empty(self.title.clone(), "title")?;
        if self.version == 0 {
            return Err(TTError::Protocol(
                "plan version must be at least 1".to_string(),
            ));
        }
        if let Some(focus) = self.current_focus_item_id.as_ref()
            && !self.plan_items.iter().any(|item| &item.item_id == focus)
        {
            return Err(TTError::Protocol(format!(
                "current focus item `{focus}` does not exist on the plan"
            )));
        }
        if self.goals.is_empty() {
            return Err(TTError::Protocol(
                "workstream plan must contain at least one goal".to_string(),
            ));
        }

        let mut goal_ids = BTreeSet::new();
        for goal in &self.goals {
            if !goal_ids.insert(goal.goal_id.clone()) {
                return Err(TTError::Protocol(format!(
                    "duplicate goal `{}` on workstream plan",
                    goal.goal_id
                )));
            }
            require_non_empty(goal.title.clone(), "goal.title")?;
            require_non_empty(goal.priority.clone(), "goal.priority")?;
        }

        let mut item_ids = BTreeSet::new();
        for item in &self.plan_items {
            if !item_ids.insert(item.item_id.clone()) {
                return Err(TTError::Protocol(format!(
                    "duplicate plan item `{}` on workstream plan",
                    item.item_id
                )));
            }
            if !goal_ids.contains(&item.goal_id) {
                return Err(TTError::Protocol(format!(
                    "plan item `{}` referenced unknown goal `{}`",
                    item.item_id, item.goal_id
                )));
            }
            require_non_empty(item.title.clone(), "plan_item.title")?;
            require_non_empty(item.priority.clone(), "plan_item.priority")?;

            let mut dependency_ids = BTreeSet::new();
            for dependency_id in &item.dependency_item_ids {
                if dependency_id == &item.item_id {
                    return Err(TTError::Protocol(format!(
                        "plan item `{}` cannot depend on itself",
                        item.item_id
                    )));
                }
                if !dependency_ids.insert(dependency_id.clone()) {
                    return Err(TTError::Protocol(format!(
                        "plan item `{}` listed dependency `{}` more than once",
                        item.item_id, dependency_id
                    )));
                }
            }
        }

        for item in &self.plan_items {
            for dependency_id in &item.dependency_item_ids {
                if !item_ids.contains(dependency_id) {
                    return Err(TTError::Protocol(format!(
                        "plan item `{}` referenced unknown dependency `{}`",
                        item.item_id, dependency_id
                    )));
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn bootstrap_from_workstream(
        workstream: &Workstream,
        work_units: &[WorkUnit],
        actor: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        let actor = actor.into();
        let goal_id = PlanGoalId::parse(format!("goal-{}", Uuid::now_v7().simple()))
            .expect("generated goal id");
        let mut plan_items = Vec::new();
        let mut sorted_units = work_units.to_vec();
        sorted_units.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        for unit in &sorted_units {
            plan_items.push(PlanItem {
                item_id: PlanItemId::parse(format!("item-{}", Uuid::now_v7().simple()))
                    .expect("generated item id"),
                goal_id: goal_id.clone(),
                title: unit.title.clone(),
                purpose: Some(unit.task_statement.clone()),
                priority: match unit.status {
                    WorkUnitStatus::Blocked => "high".to_string(),
                    WorkUnitStatus::NeedsHuman => "high".to_string(),
                    WorkUnitStatus::Completed => "low".to_string(),
                    WorkUnitStatus::Accepted
                    | WorkUnitStatus::Ready
                    | WorkUnitStatus::AwaitingDecision
                    | WorkUnitStatus::Running => "normal".to_string(),
                },
                status: match unit.status {
                    WorkUnitStatus::Ready | WorkUnitStatus::AwaitingDecision => {
                        PlanItemStatus::Pending
                    }
                    WorkUnitStatus::Running => PlanItemStatus::InProgress,
                    WorkUnitStatus::Blocked | WorkUnitStatus::NeedsHuman => PlanItemStatus::Blocked,
                    WorkUnitStatus::Completed | WorkUnitStatus::Accepted => PlanItemStatus::Done,
                },
                acceptance_criteria: Vec::new(),
                dependency_item_ids: Vec::new(),
                notes: None,
                linked_work_unit_id: Some(unit.id.clone()),
                linked_assignment_ids: unit.current_assignment_id.clone().into_iter().collect(),
                evidence_refs: unit.latest_report_id.clone().into_iter().collect(),
            });
        }

        let focus_item_id = plan_items
            .iter()
            .find(|item| {
                matches!(
                    item.status,
                    PlanItemStatus::Pending | PlanItemStatus::InProgress | PlanItemStatus::Blocked
                )
            })
            .map(|item| item.item_id.clone())
            .or_else(|| plan_items.first().map(|item| item.item_id.clone()));

        Self {
            plan_id: PlanId::parse(format!("plan-{}", Uuid::now_v7().simple()))
                .expect("generated plan id"),
            workstream_id: workstream.id.clone(),
            version: 1,
            status: PlanStatus::Active,
            title: workstream.title.clone(),
            overview: Some(workstream.objective.clone()),
            goals: vec![PlanGoal {
                goal_id,
                title: workstream.title.clone(),
                description: Some(workstream.objective.clone()),
                priority: workstream.priority.clone(),
                status: PlanGoalStatus::InProgress,
            }],
            plan_items,
            success_criteria: Vec::new(),
            constraints: Vec::new(),
            exploration_policy: ExplorationPolicy::default(),
            current_focus_item_id: focus_item_id,
            created_at: now,
            updated_at: now,
            created_by: actor.clone(),
            updated_by: actor,
            superseded_by_plan_id: None,
            source_revision_proposal_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanAssessment {
    pub assessment_id: PlanAssessmentId,
    pub workstream_id: String,
    pub plan_id: PlanId,
    pub plan_version: u64,
    #[serde(default)]
    pub assignment_id: Option<String>,
    #[serde(default)]
    pub plan_item_id: Option<PlanItemId>,
    pub alignment_status: AlignmentStatus,
    pub progress_summary: String,
    pub drift_risk: DriftRisk,
    #[serde(default)]
    pub blocker_summary: Option<String>,
    pub recommended_next_action: String,
    pub proposed_revision_needed: bool,
    pub execution_kind: PlanExecutionKind,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanRevisionProposalStatus {
    #[default]
    Pending,
    Approved,
    Applying,
    ApplyFailed,
    Rejected,
    Applied,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanRevisionApplyPhase {
    #[default]
    NotStarted,
    DownstreamApplying,
    AwaitingFinalization,
    Applied,
    FailedBeforeDownstream,
    FailedDuringDownstream,
    FailedAfterDownstream,
    Rejected,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanRevisionApplyFailureKind {
    #[default]
    RetryableInfrastructure,
    ValidationFailure,
    StaleBasePlan,
    DownstreamUnknown,
    FinalizationFailure,
    OperatorRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRevisionRecoveryState {
    #[serde(default)]
    pub phase: PlanRevisionApplyPhase,
    #[serde(default)]
    pub failure_kind: Option<PlanRevisionApplyFailureKind>,
    #[serde(default)]
    pub downstream_apply_started: bool,
    #[serde(default)]
    pub downstream_apply_completed: bool,
    #[serde(default)]
    pub retry_safe: bool,
    #[serde(default)]
    pub reconcile_available: bool,
    #[serde(default)]
    pub operator_intervention_required: bool,
    #[serde(default)]
    pub failure_message: Option<String>,
    #[serde(default)]
    pub downstream_decision_id: Option<String>,
    #[serde(default)]
    pub downstream_assignment_id: Option<String>,
}

impl Default for PlanRevisionRecoveryState {
    fn default() -> Self {
        Self {
            phase: PlanRevisionApplyPhase::NotStarted,
            failure_kind: None,
            downstream_apply_started: false,
            downstream_apply_completed: false,
            retry_safe: false,
            reconcile_available: false,
            operator_intervention_required: false,
            failure_message: None,
            downstream_decision_id: None,
            downstream_assignment_id: None,
        }
    }
}

impl PlanRevisionRecoveryState {
    #[must_use]
    pub fn can_retry(&self) -> bool {
        self.retry_safe && !self.reconcile_available && !self.operator_intervention_required
    }

    #[must_use]
    pub fn can_reconcile(&self) -> bool {
        self.reconcile_available && self.downstream_apply_completed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanRevisionOp {
    AddGoal {
        goal: PlanGoal,
    },
    UpdateGoal {
        goal_id: PlanGoalId,
        patch: PlanGoalPatch,
    },
    RemoveGoal {
        goal_id: PlanGoalId,
    },
    AddItem {
        item: PlanItem,
    },
    UpdateItem {
        item_id: PlanItemId,
        patch: PlanItemPatch,
    },
    MoveItemBefore {
        item_id: PlanItemId,
        before_item_id: Option<PlanItemId>,
    },
    RemoveItem {
        item_id: PlanItemId,
    },
    SetCurrentFocus {
        item_id: Option<PlanItemId>,
    },
    UpdateSuccessCriteria {
        success_criteria: Vec<String>,
    },
    UpdateConstraints {
        constraints: Vec<String>,
    },
    UpdateExplorationPolicy {
        exploration_policy: ExplorationPolicy,
    },
    SplitItem {
        source_item_id: PlanItemId,
        new_items: Vec<PlanItem>,
    },
    MergeItems {
        source_item_ids: Vec<PlanItemId>,
        merged_item: PlanItem,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanGoalPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub status: Option<PlanGoalStatus>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanItemPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub purpose: Option<Option<String>>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub status: Option<PlanItemStatus>,
    #[serde(default)]
    pub acceptance_criteria: Option<Vec<String>>,
    #[serde(default)]
    pub dependency_item_ids: Option<Vec<PlanItemId>>,
    #[serde(default)]
    pub notes: Option<Option<String>>,
    #[serde(default)]
    pub linked_work_unit_id: Option<Option<String>>,
    #[serde(default)]
    pub linked_assignment_ids: Option<Vec<String>>,
    #[serde(default)]
    pub evidence_refs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRevisionProposal {
    pub proposal_id: PlanRevisionProposalId,
    pub workstream_id: String,
    pub base_plan_id: PlanId,
    pub base_plan_version: u64,
    pub rationale: String,
    pub urgency: String,
    pub expected_benefit: String,
    pub tradeoffs: Vec<String>,
    #[serde(default)]
    pub ops: Vec<PlanRevisionOp>,
    #[serde(default)]
    pub status: PlanRevisionProposalStatus,
    #[serde(default = "default_utc_now")]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub reviewed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reviewed_by: Option<String>,
    #[serde(default)]
    pub review_note: Option<String>,
    #[serde(default)]
    pub apply_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub apply_finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub apply_error: Option<String>,
    #[serde(default)]
    pub recovery: PlanRevisionRecoveryState,
    #[serde(default)]
    pub applied_plan_id: Option<PlanId>,
    #[serde(default)]
    pub applied_plan_version: Option<u64>,
    #[serde(default)]
    pub source_supervisor_proposal_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanningState {
    #[serde(default)]
    pub workstream_plans: BTreeMap<String, Vec<WorkstreamPlan>>,
    #[serde(default)]
    pub assessments: BTreeMap<String, PlanAssessment>,
    #[serde(default)]
    pub revision_proposals: BTreeMap<String, PlanRevisionProposal>,
}

impl PlanningState {
    #[must_use]
    pub fn active_plan(&self, workstream_id: &str) -> Option<&WorkstreamPlan> {
        self.workstream_plans.get(workstream_id)?.last()
    }

    #[must_use]
    pub fn active_plan_mut(&mut self, workstream_id: &str) -> Option<&mut WorkstreamPlan> {
        self.workstream_plans.get_mut(workstream_id)?.last_mut()
    }

    pub fn bootstrap_workstream(
        &mut self,
        workstream: &Workstream,
        work_units: &[WorkUnit],
        actor: impl Into<String>,
        now: DateTime<Utc>,
    ) -> TTResult<Option<WorkstreamPlan>> {
        if self.workstream_plans.contains_key(&workstream.id) {
            return Ok(None);
        }
        let plan = WorkstreamPlan::bootstrap_from_workstream(workstream, work_units, actor, now);
        plan.validate()?;
        self.workstream_plans
            .insert(workstream.id.clone(), vec![plan.clone()]);
        Ok(Some(plan))
    }

    pub fn active_plan_version(&self, workstream_id: &str) -> Option<u64> {
        self.active_plan(workstream_id).map(|plan| plan.version)
    }

    pub fn record_assessment(&mut self, assessment: PlanAssessment) {
        self.assessments
            .insert(assessment.assessment_id.to_string(), assessment);
    }

    pub fn pending_revision_proposals_for_workstream(
        &self,
        workstream_id: &str,
    ) -> Vec<&PlanRevisionProposal> {
        let mut proposals = self
            .revision_proposals
            .values()
            .filter(|proposal| {
                proposal.workstream_id == workstream_id
                    && proposal.status == PlanRevisionProposalStatus::Pending
            })
            .collect::<Vec<_>>();
        proposals.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.proposal_id.as_str().cmp(right.proposal_id.as_str()))
        });
        proposals
    }

    pub fn recent_assessments_for_workstream(
        &self,
        workstream_id: &str,
        limit: usize,
    ) -> Vec<PlanAssessment> {
        let mut assessments = self
            .assessments
            .values()
            .filter(|assessment| assessment.workstream_id == workstream_id)
            .cloned()
            .collect::<Vec<_>>();
        assessments.sort_by(|left, right| {
            right.created_at.cmp(&left.created_at).then_with(|| {
                left.assessment_id
                    .as_str()
                    .cmp(right.assessment_id.as_str())
            })
        });
        assessments.truncate(limit);
        assessments
    }

    pub fn propose_revision(
        &mut self,
        proposal: PlanRevisionProposal,
    ) -> TTResult<PlanRevisionProposal> {
        if self
            .revision_proposals
            .contains_key(proposal.proposal_id.as_str())
        {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` already exists",
                proposal.proposal_id
            )));
        }
        if self
            .active_plan(&proposal.workstream_id)
            .is_none_or(|plan| plan.plan_id != proposal.base_plan_id)
        {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` targets a stale or unknown base plan",
                proposal.proposal_id
            )));
        }
        let active_version = self
            .active_plan_version(&proposal.workstream_id)
            .ok_or_else(|| {
                TTError::Protocol(format!(
                    "unknown workstream `{}` for plan revision proposal",
                    proposal.workstream_id
                ))
            })?;
        if active_version != proposal.base_plan_version {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` targets stale plan version {}",
                proposal.proposal_id, proposal.base_plan_version
            )));
        }
        let active_plan = self.active_plan(&proposal.workstream_id).ok_or_else(|| {
            TTError::Protocol(format!(
                "unknown workstream `{}` for plan revision proposal",
                proposal.workstream_id
            ))
        })?;
        validate_plan_revision_ops(active_plan, &proposal.ops)?;
        self.revision_proposals
            .insert(proposal.proposal_id.to_string(), proposal.clone());
        Ok(proposal)
    }

    fn revision_proposal_mut(
        &mut self,
        proposal_id: &PlanRevisionProposalId,
    ) -> TTResult<&mut PlanRevisionProposal> {
        self.revision_proposals
            .get_mut(proposal_id.as_str())
            .ok_or_else(|| {
                TTError::Protocol(format!("unknown plan revision proposal `{}`", proposal_id))
            })
    }

    fn revision_proposal_snapshot(
        &self,
        proposal_id: &PlanRevisionProposalId,
    ) -> TTResult<PlanRevisionProposal> {
        self.revision_proposals
            .get(proposal_id.as_str())
            .cloned()
            .ok_or_else(|| {
                TTError::Protocol(format!("unknown plan revision proposal `{}`", proposal_id))
            })
    }

    fn mark_revision_retrying(
        proposal: &mut PlanRevisionProposal,
        now: DateTime<Utc>,
        reviewed_by: impl Into<String>,
        review_note: Option<String>,
    ) {
        let reviewed_by = reviewed_by.into();
        proposal.status = PlanRevisionProposalStatus::Applying;
        proposal.reviewed_at = Some(now);
        proposal.reviewed_by = Some(reviewed_by);
        proposal.review_note = review_note;
        proposal.apply_started_at = Some(now);
        proposal.apply_finished_at = None;
        proposal.apply_error = None;
        proposal.recovery.phase = PlanRevisionApplyPhase::DownstreamApplying;
        proposal.recovery.failure_kind = None;
        proposal.recovery.retry_safe = false;
        proposal.recovery.reconcile_available = false;
        proposal.recovery.operator_intervention_required = false;
        proposal.recovery.failure_message = None;
        proposal.recovery.downstream_apply_started = true;
        proposal.recovery.downstream_apply_completed = false;
        proposal.recovery.downstream_decision_id = None;
        proposal.recovery.downstream_assignment_id = None;
        proposal.applied_plan_id = None;
        proposal.applied_plan_version = None;
    }

    pub fn begin_apply_revision(
        &mut self,
        proposal_id: &PlanRevisionProposalId,
        reviewed_by: impl Into<String>,
        review_note: Option<String>,
        now: DateTime<Utc>,
    ) -> TTResult<PlanRevisionProposal> {
        let reviewed_by = reviewed_by.into();
        let proposal_snapshot = self.revision_proposal_snapshot(proposal_id)?;
        if !matches!(
            proposal_snapshot.status,
            PlanRevisionProposalStatus::Pending | PlanRevisionProposalStatus::ApplyFailed
        ) {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` is not pending or retryable",
                proposal_id
            )));
        }
        let active_plan = self
            .active_plan(&proposal_snapshot.workstream_id)
            .cloned()
            .ok_or_else(|| {
                TTError::Protocol(format!(
                    "unknown workstream `{}` for plan revision approval",
                    proposal_snapshot.workstream_id
                ))
            })?;
        if active_plan.plan_id != proposal_snapshot.base_plan_id
            || active_plan.version != proposal_snapshot.base_plan_version
        {
            if let Some(proposal) = self.revision_proposals.get_mut(proposal_id.as_str()) {
                proposal.status = PlanRevisionProposalStatus::Superseded;
                proposal.reviewed_at = Some(now);
                proposal.reviewed_by = Some(reviewed_by.clone());
                proposal.review_note =
                    review_note.or_else(|| Some("Base plan changed before approval.".to_string()));
                proposal.apply_finished_at = Some(now);
                proposal.apply_error =
                    Some("Base plan changed before revision application.".to_string());
                proposal.recovery.phase = PlanRevisionApplyPhase::Superseded;
                proposal.recovery.failure_kind = Some(PlanRevisionApplyFailureKind::StaleBasePlan);
                proposal.recovery.retry_safe = false;
                proposal.recovery.reconcile_available = false;
                proposal.recovery.operator_intervention_required = false;
                proposal.recovery.failure_message = proposal.apply_error.clone();
                proposal.recovery.downstream_apply_started = false;
                proposal.recovery.downstream_apply_completed = false;
            }
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` is stale",
                proposal_id
            )));
        }
        validate_plan_revision_ops(&active_plan, &proposal_snapshot.ops)?;
        let proposal = self.revision_proposal_mut(proposal_id)?;
        if matches!(proposal.status, PlanRevisionProposalStatus::ApplyFailed) {
            if !proposal.recovery.can_retry() {
                return Err(TTError::Protocol(format!(
                    "plan revision proposal `{}` is not safely retryable",
                    proposal_id
                )));
            }
            Self::mark_revision_retrying(proposal, now, reviewed_by, review_note);
        } else {
            proposal.status = PlanRevisionProposalStatus::Applying;
            proposal.reviewed_at = Some(now);
            proposal.reviewed_by = Some(reviewed_by);
            proposal.review_note = review_note;
            proposal.apply_started_at = Some(now);
            proposal.apply_finished_at = None;
            proposal.apply_error = None;
            proposal.recovery.phase = PlanRevisionApplyPhase::DownstreamApplying;
            proposal.recovery.failure_kind = None;
            proposal.recovery.retry_safe = false;
            proposal.recovery.reconcile_available = false;
            proposal.recovery.operator_intervention_required = false;
            proposal.recovery.failure_message = None;
            proposal.recovery.downstream_apply_started = true;
            proposal.recovery.downstream_apply_completed = false;
            proposal.recovery.downstream_decision_id = None;
            proposal.recovery.downstream_assignment_id = None;
            proposal.applied_plan_id = None;
            proposal.applied_plan_version = None;
        }
        Ok(proposal.clone())
    }

    pub fn complete_apply_revision(
        &mut self,
        proposal_id: &PlanRevisionProposalId,
        reviewed_by: impl Into<String>,
        review_note: Option<String>,
        now: DateTime<Utc>,
    ) -> TTResult<WorkstreamPlan> {
        let reviewed_by = reviewed_by.into();
        let proposal_snapshot = self.revision_proposal_snapshot(proposal_id)?;
        if !matches!(
            proposal_snapshot.status,
            PlanRevisionProposalStatus::Applying
                | PlanRevisionProposalStatus::ApplyFailed
                | PlanRevisionProposalStatus::Applied
        ) {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` is not applying or reconcilable",
                proposal_id
            )));
        }
        let active_plan = self
            .active_plan(&proposal_snapshot.workstream_id)
            .cloned()
            .ok_or_else(|| {
                TTError::Protocol(format!(
                    "unknown workstream `{}` for plan revision approval",
                    proposal_snapshot.workstream_id
                ))
            })?;
        if let Some(applied_plan) = self
            .workstream_plans
            .get(&proposal_snapshot.workstream_id)
            .and_then(|series| {
                series
                    .iter()
                    .rev()
                    .find(|plan| plan.source_revision_proposal_id.as_ref() == Some(proposal_id))
            })
            .cloned()
        {
            if let Some(proposal) = self.revision_proposals.get_mut(proposal_id.as_str()) {
                proposal.status = PlanRevisionProposalStatus::Applied;
                proposal.reviewed_at = Some(now);
                proposal.reviewed_by = Some(reviewed_by.clone());
                proposal.review_note = review_note;
                proposal.apply_finished_at = Some(now);
                proposal.apply_error = None;
                proposal.recovery.phase = PlanRevisionApplyPhase::Applied;
                proposal.recovery.failure_kind = None;
                proposal.recovery.retry_safe = false;
                proposal.recovery.reconcile_available = false;
                proposal.recovery.operator_intervention_required = false;
                proposal.recovery.failure_message = None;
                proposal.recovery.downstream_apply_started = true;
                proposal.recovery.downstream_apply_completed = true;
                proposal.applied_plan_id = Some(applied_plan.plan_id.clone());
                proposal.applied_plan_version = Some(applied_plan.version);
            }
            return Ok(applied_plan);
        }

        if active_plan.plan_id != proposal_snapshot.base_plan_id
            || active_plan.version != proposal_snapshot.base_plan_version
        {
            if let Some(proposal) = self.revision_proposals.get_mut(proposal_id.as_str()) {
                proposal.status = PlanRevisionProposalStatus::Superseded;
                proposal.reviewed_at = Some(now);
                proposal.reviewed_by = Some(reviewed_by);
                proposal.review_note =
                    review_note.or_else(|| Some("Base plan changed before approval.".to_string()));
                proposal.apply_finished_at = Some(now);
                proposal.apply_error =
                    Some("Base plan changed before revision application.".to_string());
                proposal.recovery.phase = PlanRevisionApplyPhase::Superseded;
                proposal.recovery.failure_kind = Some(PlanRevisionApplyFailureKind::StaleBasePlan);
                proposal.recovery.retry_safe = false;
                proposal.recovery.reconcile_available = false;
                proposal.recovery.operator_intervention_required = false;
                proposal.recovery.failure_message = proposal.apply_error.clone();
                proposal.recovery.downstream_apply_started = false;
                proposal.recovery.downstream_apply_completed = false;
            }
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` is stale",
                proposal_id
            )));
        }

        if proposal_snapshot.status == PlanRevisionProposalStatus::ApplyFailed
            && !proposal_snapshot.recovery.reconcile_available
        {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` is not reconcilable",
                proposal_id
            )));
        }

        let mut updated = validate_plan_revision_ops(&active_plan, &proposal_snapshot.ops)?;
        updated.version = active_plan.version.saturating_add(1);
        updated.plan_id =
            PlanId::parse(format!("plan-{}", Uuid::now_v7().simple())).expect("generated plan id");
        updated.status = PlanStatus::Active;
        updated.updated_at = now;
        updated.updated_by = reviewed_by.clone();
        updated.superseded_by_plan_id = None;
        updated.source_revision_proposal_id = Some(proposal_snapshot.proposal_id.clone());

        if let Some(series) = self
            .workstream_plans
            .get_mut(&proposal_snapshot.workstream_id)
        {
            if let Some(previous) = series.last_mut() {
                previous.status = PlanStatus::Superseded;
                previous.updated_at = now;
                previous.updated_by = reviewed_by.clone();
                previous.superseded_by_plan_id = Some(updated.plan_id.clone());
            }
            series.push(updated.clone());
        } else {
            return Err(TTError::Protocol(format!(
                "unknown workstream `{}` for plan revision approval",
                proposal_snapshot.workstream_id
            )));
        }

        if let Some(proposal) = self.revision_proposals.get_mut(proposal_id.as_str()) {
            proposal.status = PlanRevisionProposalStatus::Applied;
            proposal.reviewed_at = Some(now);
            proposal.reviewed_by = Some(reviewed_by);
            proposal.review_note = review_note;
            proposal.apply_finished_at = Some(now);
            proposal.apply_error = None;
            proposal.recovery.phase = PlanRevisionApplyPhase::Applied;
            proposal.recovery.failure_kind = None;
            proposal.recovery.retry_safe = false;
            proposal.recovery.reconcile_available = false;
            proposal.recovery.operator_intervention_required = false;
            proposal.recovery.failure_message = None;
            proposal.recovery.downstream_apply_started = true;
            proposal.recovery.downstream_apply_completed = true;
            proposal.applied_plan_id = Some(updated.plan_id.clone());
            proposal.applied_plan_version = Some(updated.version);
        }
        Ok(updated)
    }

    pub fn record_downstream_completion(
        &mut self,
        proposal_id: &PlanRevisionProposalId,
        decision_id: impl Into<String>,
        assignment_id: Option<String>,
        now: DateTime<Utc>,
    ) -> TTResult<PlanRevisionProposal> {
        let proposal = self.revision_proposal_mut(proposal_id)?;
        if !matches!(
            proposal.status,
            PlanRevisionProposalStatus::Applying | PlanRevisionProposalStatus::ApplyFailed
        ) {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` is not applying or recoverable",
                proposal_id
            )));
        }
        proposal.recovery.phase = PlanRevisionApplyPhase::AwaitingFinalization;
        proposal.recovery.downstream_apply_started = true;
        proposal.recovery.downstream_apply_completed = true;
        proposal.recovery.downstream_decision_id = Some(decision_id.into());
        proposal.recovery.downstream_assignment_id = assignment_id;
        proposal.recovery.failure_kind = None;
        proposal.recovery.retry_safe = false;
        proposal.recovery.reconcile_available = true;
        proposal.recovery.operator_intervention_required = false;
        proposal.recovery.failure_message = None;
        proposal.apply_started_at.get_or_insert(now);
        proposal.apply_error = None;
        Ok(proposal.clone())
    }

    pub fn fail_apply_revision(
        &mut self,
        proposal_id: &PlanRevisionProposalId,
        reviewed_by: impl Into<String>,
        review_note: Option<String>,
        phase: PlanRevisionApplyPhase,
        failure_kind: PlanRevisionApplyFailureKind,
        retry_safe: bool,
        reconcile_available: bool,
        operator_intervention_required: bool,
        apply_error: impl Into<String>,
        now: DateTime<Utc>,
    ) -> TTResult<PlanRevisionProposal> {
        let reviewed_by = reviewed_by.into();
        let proposal = self.revision_proposal_mut(proposal_id)?;
        if !matches!(
            proposal.status,
            PlanRevisionProposalStatus::Applying
                | PlanRevisionProposalStatus::ApplyFailed
                | PlanRevisionProposalStatus::Applied
        ) {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` is not applying or recoverable",
                proposal_id
            )));
        }
        proposal.status = PlanRevisionProposalStatus::ApplyFailed;
        proposal.reviewed_at = Some(now);
        proposal.reviewed_by = Some(reviewed_by);
        proposal.review_note = review_note;
        proposal.apply_finished_at = Some(now);
        proposal.apply_error = Some(apply_error.into());
        proposal.recovery.phase = phase;
        proposal.recovery.failure_kind = Some(failure_kind);
        proposal.recovery.retry_safe = retry_safe;
        proposal.recovery.reconcile_available = reconcile_available;
        proposal.recovery.operator_intervention_required = operator_intervention_required;
        proposal.recovery.failure_message = proposal.apply_error.clone();
        proposal.recovery.downstream_apply_started = matches!(
            phase,
            PlanRevisionApplyPhase::DownstreamApplying
                | PlanRevisionApplyPhase::AwaitingFinalization
                | PlanRevisionApplyPhase::Applied
                | PlanRevisionApplyPhase::FailedDuringDownstream
                | PlanRevisionApplyPhase::FailedAfterDownstream
        );
        proposal.recovery.downstream_apply_completed = matches!(
            phase,
            PlanRevisionApplyPhase::AwaitingFinalization
                | PlanRevisionApplyPhase::Applied
                | PlanRevisionApplyPhase::FailedAfterDownstream
        );
        Ok(proposal.clone())
    }

    pub fn reject_revision(
        &mut self,
        proposal_id: &PlanRevisionProposalId,
        reviewed_by: impl Into<String>,
        review_note: Option<String>,
        now: DateTime<Utc>,
    ) -> TTResult<PlanRevisionProposal> {
        let reviewed_by = reviewed_by.into();
        let proposal = self
            .revision_proposals
            .get_mut(proposal_id.as_str())
            .ok_or_else(|| {
                TTError::Protocol(format!("unknown plan revision proposal `{}`", proposal_id))
            })?;
        if !matches!(
            proposal.status,
            PlanRevisionProposalStatus::Pending | PlanRevisionProposalStatus::ApplyFailed
        ) {
            return Err(TTError::Protocol(format!(
                "plan revision proposal `{}` cannot be rejected from status `{:?}`",
                proposal_id, proposal.status
            )));
        }
        proposal.status = PlanRevisionProposalStatus::Rejected;
        proposal.reviewed_at = Some(now);
        proposal.reviewed_by = Some(reviewed_by);
        proposal.review_note = review_note;
        proposal.apply_finished_at = Some(now);
        Ok(proposal.clone())
    }
}

pub fn validate_plan_revision_ops(
    base: &WorkstreamPlan,
    ops: &[PlanRevisionOp],
) -> TTResult<WorkstreamPlan> {
    if ops.is_empty() {
        return Err(TTError::Protocol(
            "plan revision proposal must include at least one operation".to_string(),
        ));
    }
    let mut updated = base.clone();
    apply_plan_revision_ops(base, &mut updated, ops)?;
    updated.validate()?;
    Ok(updated)
}

fn apply_plan_revision_ops(
    base: &WorkstreamPlan,
    updated: &mut WorkstreamPlan,
    ops: &[PlanRevisionOp],
) -> TTResult<()> {
    for op in ops {
        match op {
            PlanRevisionOp::AddGoal { goal } => {
                require_non_empty(goal.title.clone(), "goal.title")?;
                require_non_empty(goal.priority.clone(), "goal.priority")?;
                if updated
                    .goals
                    .iter()
                    .any(|existing| existing.goal_id == goal.goal_id)
                {
                    return Err(TTError::Protocol(format!(
                        "goal `{}` already exists",
                        goal.goal_id
                    )));
                }
                updated.goals.push(goal.clone());
            }
            PlanRevisionOp::UpdateGoal { goal_id, patch } => {
                let goal = updated
                    .goals
                    .iter_mut()
                    .find(|goal| &goal.goal_id == goal_id)
                    .ok_or_else(|| TTError::Protocol(format!("unknown goal `{goal_id}`")))?;
                if let Some(title) = patch.title.as_ref() {
                    goal.title = require_non_empty(title.clone(), "goal.title")?;
                }
                if let Some(description) = patch.description.as_ref() {
                    goal.description = description.clone();
                }
                if let Some(priority) = patch.priority.as_ref() {
                    goal.priority = require_non_empty(priority.clone(), "goal.priority")?;
                }
                if let Some(status) = patch.status {
                    goal.status = status;
                }
            }
            PlanRevisionOp::RemoveGoal { goal_id } => {
                if !updated.goals.iter().any(|goal| &goal.goal_id == goal_id) {
                    return Err(TTError::Protocol(format!("unknown goal `{goal_id}`")));
                }
                if updated.goals.len() == 1 {
                    return Err(TTError::Protocol(
                        "plan revision cannot remove the final goal".to_string(),
                    ));
                }
                updated.goals.retain(|goal| &goal.goal_id != goal_id);
                updated.plan_items.retain(|item| &item.goal_id != goal_id);
            }
            PlanRevisionOp::AddItem { item } => {
                require_non_empty(item.title.clone(), "plan_item.title")?;
                require_non_empty(item.priority.clone(), "plan_item.priority")?;
                if !updated
                    .goals
                    .iter()
                    .any(|goal| goal.goal_id == item.goal_id)
                {
                    return Err(TTError::Protocol(format!(
                        "plan item `{}` referenced unknown goal `{}`",
                        item.item_id, item.goal_id
                    )));
                }
                if updated
                    .plan_items
                    .iter()
                    .any(|existing| existing.item_id == item.item_id)
                {
                    return Err(TTError::Protocol(format!(
                        "plan item `{}` already exists",
                        item.item_id
                    )));
                }
                updated.plan_items.push(item.clone());
            }
            PlanRevisionOp::UpdateItem { item_id, patch } => {
                let item = updated
                    .plan_items
                    .iter_mut()
                    .find(|item| &item.item_id == item_id)
                    .ok_or_else(|| TTError::Protocol(format!("unknown plan item `{item_id}`")))?;
                if let Some(title) = patch.title.as_ref() {
                    item.title = require_non_empty(title.clone(), "plan_item.title")?;
                }
                if let Some(purpose) = patch.purpose.as_ref() {
                    item.purpose = purpose.clone();
                }
                if let Some(priority) = patch.priority.as_ref() {
                    item.priority = require_non_empty(priority.clone(), "plan_item.priority")?;
                }
                if let Some(status) = patch.status {
                    item.status = status;
                }
                if let Some(criteria) = patch.acceptance_criteria.as_ref() {
                    item.acceptance_criteria = criteria.clone();
                }
                if let Some(dependencies) = patch.dependency_item_ids.as_ref() {
                    item.dependency_item_ids = dependencies.clone();
                }
                if let Some(notes) = patch.notes.as_ref() {
                    item.notes = notes.clone();
                }
                if let Some(linked_work_unit_id) = patch.linked_work_unit_id.as_ref() {
                    item.linked_work_unit_id = linked_work_unit_id.clone();
                }
                if let Some(linked_assignment_ids) = patch.linked_assignment_ids.as_ref() {
                    item.linked_assignment_ids = linked_assignment_ids.clone();
                }
                if let Some(evidence_refs) = patch.evidence_refs.as_ref() {
                    item.evidence_refs = evidence_refs.clone();
                }
            }
            PlanRevisionOp::MoveItemBefore {
                item_id,
                before_item_id,
            } => {
                if before_item_id.as_ref() == Some(item_id) {
                    return Err(TTError::Protocol(format!(
                        "plan item `{item_id}` cannot be moved before itself"
                    )));
                }
                let index = updated
                    .plan_items
                    .iter()
                    .position(|item| &item.item_id == item_id)
                    .ok_or_else(|| TTError::Protocol(format!("unknown plan item `{item_id}`")))?;
                let item = updated.plan_items.remove(index);
                let insert_at = match before_item_id {
                    Some(before_item_id) => updated
                        .plan_items
                        .iter()
                        .position(|item| &item.item_id == before_item_id)
                        .ok_or_else(|| {
                            TTError::Protocol(format!("unknown plan item `{before_item_id}`"))
                        })?,
                    None => updated.plan_items.len(),
                };
                updated.plan_items.insert(insert_at, item);
            }
            PlanRevisionOp::RemoveItem { item_id } => {
                updated.plan_items.retain(|item| &item.item_id != item_id);
                if updated.current_focus_item_id.as_ref() == Some(item_id) {
                    updated.current_focus_item_id = None;
                }
            }
            PlanRevisionOp::SetCurrentFocus { item_id } => {
                if let Some(item_id) = item_id
                    && !updated
                        .plan_items
                        .iter()
                        .any(|item| &item.item_id == item_id)
                {
                    return Err(TTError::Protocol(format!(
                        "unknown current focus item `{item_id}`"
                    )));
                }
                updated.current_focus_item_id = item_id.clone();
            }
            PlanRevisionOp::UpdateSuccessCriteria { success_criteria } => {
                updated.success_criteria = success_criteria.clone();
            }
            PlanRevisionOp::UpdateConstraints { constraints } => {
                updated.constraints = constraints.clone();
            }
            PlanRevisionOp::UpdateExplorationPolicy { exploration_policy } => {
                updated.exploration_policy = exploration_policy.clone();
            }
            PlanRevisionOp::SplitItem {
                source_item_id,
                new_items,
            } => {
                if new_items.is_empty() {
                    return Err(TTError::Protocol(format!(
                        "split item `{source_item_id}` must create at least one replacement item"
                    )));
                }
                let source_exists = updated
                    .plan_items
                    .iter()
                    .any(|item| &item.item_id == source_item_id);
                if !source_exists {
                    return Err(TTError::Protocol(format!(
                        "unknown source item `{source_item_id}`"
                    )));
                }
                updated
                    .plan_items
                    .retain(|item| &item.item_id != source_item_id);
                for item in new_items {
                    require_non_empty(item.title.clone(), "plan_item.title")?;
                    require_non_empty(item.priority.clone(), "plan_item.priority")?;
                    if !updated
                        .goals
                        .iter()
                        .any(|goal| goal.goal_id == item.goal_id)
                    {
                        return Err(TTError::Protocol(format!(
                            "split item `{}` referenced unknown goal `{}`",
                            item.item_id, item.goal_id
                        )));
                    }
                    if updated
                        .plan_items
                        .iter()
                        .any(|existing| existing.item_id == item.item_id)
                    {
                        return Err(TTError::Protocol(format!(
                            "split item `{}` already exists",
                            item.item_id
                        )));
                    }
                    updated.plan_items.push(item.clone());
                }
                if updated.current_focus_item_id.as_ref() == Some(source_item_id) {
                    updated.current_focus_item_id =
                        new_items.first().map(|item| item.item_id.clone());
                }
            }
            PlanRevisionOp::MergeItems {
                source_item_ids,
                merged_item,
            } => {
                if source_item_ids.len() < 2 {
                    return Err(TTError::Protocol(
                        "merge items requires at least two source items".to_string(),
                    ));
                }
                let mut seen_source_ids = BTreeSet::new();
                for source_item_id in source_item_ids {
                    if !seen_source_ids.insert(source_item_id.clone()) {
                        return Err(TTError::Protocol(format!(
                            "merge items referenced source item `{source_item_id}` more than once"
                        )));
                    }
                    if !updated
                        .plan_items
                        .iter()
                        .any(|item| &item.item_id == source_item_id)
                    {
                        return Err(TTError::Protocol(format!(
                            "unknown source item `{source_item_id}`"
                        )));
                    }
                }
                require_non_empty(merged_item.title.clone(), "plan_item.title")?;
                require_non_empty(merged_item.priority.clone(), "plan_item.priority")?;
                if !updated
                    .goals
                    .iter()
                    .any(|goal| goal.goal_id == merged_item.goal_id)
                {
                    return Err(TTError::Protocol(format!(
                        "merged item `{}` referenced unknown goal `{}`",
                        merged_item.item_id, merged_item.goal_id
                    )));
                }
                updated
                    .plan_items
                    .retain(|item| !source_item_ids.contains(&item.item_id));
                if updated
                    .plan_items
                    .iter()
                    .any(|item| item.item_id == merged_item.item_id)
                {
                    return Err(TTError::Protocol(format!(
                        "merged item `{}` already exists",
                        merged_item.item_id
                    )));
                }
                updated.plan_items.push(merged_item.clone());
                if source_item_ids
                    .iter()
                    .any(|id| base.current_focus_item_id.as_ref() == Some(id))
                {
                    updated.current_focus_item_id = Some(merged_item.item_id.clone());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plan() -> WorkstreamPlan {
        let now = Utc::now();
        let goal_id = PlanGoalId::parse("goal-1").expect("goal id");
        WorkstreamPlan {
            plan_id: PlanId::parse("plan-1").expect("plan id"),
            workstream_id: "ws-1".to_string(),
            version: 1,
            status: PlanStatus::Active,
            title: "Sample plan".to_string(),
            overview: Some("overview".to_string()),
            goals: vec![PlanGoal {
                goal_id: goal_id.clone(),
                title: "Goal".to_string(),
                description: Some("desc".to_string()),
                priority: "high".to_string(),
                status: PlanGoalStatus::InProgress,
            }],
            plan_items: vec![
                PlanItem {
                    item_id: PlanItemId::parse("item-1").expect("item 1"),
                    goal_id: goal_id.clone(),
                    title: "Item one".to_string(),
                    purpose: Some("purpose".to_string()),
                    priority: "high".to_string(),
                    status: PlanItemStatus::Pending,
                    acceptance_criteria: vec!["criterion".to_string()],
                    dependency_item_ids: Vec::new(),
                    notes: None,
                    linked_work_unit_id: Some("wu-1".to_string()),
                    linked_assignment_ids: Vec::new(),
                    evidence_refs: Vec::new(),
                },
                PlanItem {
                    item_id: PlanItemId::parse("item-2").expect("item 2"),
                    goal_id,
                    title: "Item two".to_string(),
                    purpose: Some("purpose".to_string()),
                    priority: "normal".to_string(),
                    status: PlanItemStatus::Pending,
                    acceptance_criteria: vec!["criterion".to_string()],
                    dependency_item_ids: vec![PlanItemId::parse("item-1").expect("dep item")],
                    notes: None,
                    linked_work_unit_id: Some("wu-2".to_string()),
                    linked_assignment_ids: Vec::new(),
                    evidence_refs: Vec::new(),
                },
            ],
            success_criteria: vec!["done".to_string()],
            constraints: vec!["constraint".to_string()],
            exploration_policy: ExplorationPolicy::default(),
            current_focus_item_id: Some(PlanItemId::parse("item-1").expect("focus item")),
            created_at: now,
            updated_at: now,
            created_by: "tester".to_string(),
            updated_by: "tester".to_string(),
            superseded_by_plan_id: None,
            source_revision_proposal_id: None,
        }
    }

    fn sample_planning_state() -> PlanningState {
        let plan = sample_plan();
        let mut state = PlanningState::default();
        state
            .workstream_plans
            .insert(plan.workstream_id.clone(), vec![plan]);
        state
    }

    fn sample_revision(ops: Vec<PlanRevisionOp>) -> PlanRevisionProposal {
        let plan = sample_plan();
        PlanRevisionProposal {
            proposal_id: PlanRevisionProposalId::parse("proposal-1").expect("proposal id"),
            workstream_id: plan.workstream_id.clone(),
            base_plan_id: plan.plan_id.clone(),
            base_plan_version: plan.version,
            rationale: "need structural adjustment".to_string(),
            urgency: "medium".to_string(),
            expected_benefit: "better sequencing".to_string(),
            tradeoffs: vec!["minor churn".to_string()],
            ops,
            status: PlanRevisionProposalStatus::Pending,
            created_at: Utc::now(),
            created_by: "tester".to_string(),
            reviewed_at: None,
            reviewed_by: None,
            review_note: None,
            apply_started_at: None,
            apply_finished_at: None,
            apply_error: None,
            recovery: PlanRevisionRecoveryState::default(),
            applied_plan_id: None,
            applied_plan_version: None,
            source_supervisor_proposal_id: None,
        }
    }

    #[test]
    fn validate_plan_rejects_missing_dependency() {
        let mut plan = sample_plan();
        plan.plan_items[1].dependency_item_ids = vec![PlanItemId::parse("missing").expect("id")];
        let error = plan.validate().expect_err("missing dependency should fail");
        assert!(
            error
                .to_string()
                .contains("referenced unknown dependency `missing`")
        );
    }

    #[test]
    fn validate_plan_revision_ops_rejects_invalid_focus_target() {
        let plan = sample_plan();
        let error = validate_plan_revision_ops(
            &plan,
            &[PlanRevisionOp::SetCurrentFocus {
                item_id: Some(PlanItemId::parse("missing").expect("id")),
            }],
        )
        .expect_err("missing focus target should fail");
        assert!(
            error
                .to_string()
                .contains("unknown current focus item `missing`")
        );
    }

    #[test]
    fn begin_and_complete_revision_use_two_phase_lifecycle() {
        let mut state = sample_planning_state();
        let proposal = sample_revision(vec![PlanRevisionOp::MoveItemBefore {
            item_id: PlanItemId::parse("item-2").expect("item"),
            before_item_id: Some(PlanItemId::parse("item-1").expect("before")),
        }]);
        state.propose_revision(proposal.clone()).expect("proposal");

        let applying = state
            .begin_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("begin apply");
        assert_eq!(applying.status, PlanRevisionProposalStatus::Applying);

        let applied = state
            .complete_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("complete apply");
        assert_eq!(applied.version, 2);
        assert_eq!(applied.plan_items[0].item_id.as_str(), "item-2");
        assert_eq!(
            state.revision_proposals[proposal.proposal_id.as_str()].status,
            PlanRevisionProposalStatus::Applied
        );
    }

    #[test]
    fn stale_revision_is_marked_superseded_during_begin_apply() {
        let mut state = sample_planning_state();
        let mut proposal = sample_revision(vec![PlanRevisionOp::UpdateConstraints {
            constraints: vec!["new".to_string()],
        }]);
        proposal.base_plan_version = 0;
        state
            .revision_proposals
            .insert(proposal.proposal_id.to_string(), proposal.clone());

        let error = state
            .begin_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect_err("stale proposal should fail");
        assert!(error.to_string().contains("is stale"));
        assert_eq!(
            state.revision_proposals[proposal.proposal_id.as_str()].status,
            PlanRevisionProposalStatus::Superseded
        );
    }

    #[test]
    fn fail_apply_revision_records_structured_error() {
        let mut state = sample_planning_state();
        let proposal = sample_revision(vec![PlanRevisionOp::UpdateConstraints {
            constraints: vec!["new".to_string()],
        }]);
        state.propose_revision(proposal.clone()).expect("proposal");
        state
            .begin_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("begin apply");

        let failed = state
            .fail_apply_revision(
                &proposal.proposal_id,
                "reviewer",
                Some("apply failed".to_string()),
                PlanRevisionApplyPhase::FailedDuringDownstream,
                PlanRevisionApplyFailureKind::DownstreamUnknown,
                false,
                false,
                true,
                "downstream decision application failed",
                Utc::now(),
            )
            .expect("fail apply");
        assert_eq!(failed.status, PlanRevisionProposalStatus::ApplyFailed);
        assert_eq!(
            failed.apply_error.as_deref(),
            Some("downstream decision application failed")
        );
    }

    #[test]
    fn retry_safe_failure_can_restart_before_downstream_apply() {
        let mut state = sample_planning_state();
        let proposal = sample_revision(vec![PlanRevisionOp::UpdateConstraints {
            constraints: vec!["retry".to_string()],
        }]);
        state.propose_revision(proposal.clone()).expect("proposal");
        state
            .begin_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("begin apply");
        let failed = state
            .fail_apply_revision(
                &proposal.proposal_id,
                "reviewer",
                Some("checkpoint write failed".to_string()),
                PlanRevisionApplyPhase::FailedBeforeDownstream,
                PlanRevisionApplyFailureKind::RetryableInfrastructure,
                true,
                false,
                false,
                "checkpoint write failed",
                Utc::now(),
            )
            .expect("mark retryable failure");
        assert!(failed.recovery.retry_safe);
        assert_eq!(
            failed.recovery.phase,
            PlanRevisionApplyPhase::FailedBeforeDownstream
        );

        let retrying = state
            .begin_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("retry begin");
        assert_eq!(retrying.status, PlanRevisionProposalStatus::Applying);
        assert_eq!(
            retrying.recovery.phase,
            PlanRevisionApplyPhase::DownstreamApplying
        );
        let completed = state
            .record_downstream_completion(
                &proposal.proposal_id,
                "decision-1",
                Some("assignment-1".to_string()),
                Utc::now(),
            )
            .expect("record downstream completion");
        assert!(completed.recovery.reconcile_available);
        let applied = state
            .complete_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("complete after retry");
        assert_eq!(applied.version, 2);
        assert_eq!(
            state.revision_proposals[proposal.proposal_id.as_str()].status,
            PlanRevisionProposalStatus::Applied
        );
    }

    #[test]
    fn reconcile_after_downstream_completion_is_idempotent() {
        let mut state = sample_planning_state();
        let proposal = sample_revision(vec![PlanRevisionOp::UpdateConstraints {
            constraints: vec!["reconcile".to_string()],
        }]);
        state.propose_revision(proposal.clone()).expect("proposal");
        state
            .begin_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("begin apply");
        state
            .record_downstream_completion(
                &proposal.proposal_id,
                "decision-1",
                Some("assignment-1".to_string()),
                Utc::now(),
            )
            .expect("record downstream completion");
        state
            .fail_apply_revision(
                &proposal.proposal_id,
                "reviewer",
                Some("finalization persist failed".to_string()),
                PlanRevisionApplyPhase::FailedAfterDownstream,
                PlanRevisionApplyFailureKind::FinalizationFailure,
                false,
                true,
                false,
                "finalization persist failed",
                Utc::now(),
            )
            .expect("mark finalization failure");

        let applied = state
            .complete_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("reconcile finalization");
        assert_eq!(applied.version, 2);
        let applied_again = state
            .complete_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("idempotent reconcile");
        assert_eq!(applied_again.plan_id, applied.plan_id);
        assert_eq!(
            state.revision_proposals[proposal.proposal_id.as_str()]
                .recovery
                .phase,
            PlanRevisionApplyPhase::Applied
        );
    }

    #[test]
    fn unsafe_retry_is_blocked_when_reconcile_is_required() {
        let mut state = sample_planning_state();
        let proposal = sample_revision(vec![PlanRevisionOp::UpdateConstraints {
            constraints: vec!["blocked".to_string()],
        }]);
        state.propose_revision(proposal.clone()).expect("proposal");
        state
            .begin_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect("begin apply");
        state
            .record_downstream_completion(
                &proposal.proposal_id,
                "decision-1",
                Some("assignment-1".to_string()),
                Utc::now(),
            )
            .expect("record downstream completion");
        state
            .fail_apply_revision(
                &proposal.proposal_id,
                "reviewer",
                Some("ambiguous downstream failure".to_string()),
                PlanRevisionApplyPhase::FailedDuringDownstream,
                PlanRevisionApplyFailureKind::DownstreamUnknown,
                false,
                false,
                true,
                "ambiguous downstream failure",
                Utc::now(),
            )
            .expect("mark unsafe failure");

        let error = state
            .begin_apply_revision(&proposal.proposal_id, "reviewer", None, Utc::now())
            .expect_err("unsafe retry should fail");
        assert!(error.to_string().contains("not safely retryable"));
    }
}
