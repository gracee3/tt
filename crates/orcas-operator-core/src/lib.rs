use chrono::{DateTime, Utc};
use orcas_core::ipc::{
    NotificationDeliveryJob, NotificationDeliveryJobStatus, OperatorInboxActionKind,
    OperatorInboxItem, OperatorInboxItemStatus, OperatorInboxSourceKind,
    OperatorNotificationCandidate, OperatorNotificationCandidateStatus,
    OperatorRemoteActionRequest, OperatorRemoteActionRequestStatus,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorServerSettings {
    pub server_url: String,
    #[serde(default)]
    pub operator_api_token: Option<String>,
    #[serde(default)]
    pub push_public_key: Option<String>,
    #[serde(default)]
    pub origin_node_id: String,
}

impl Default for OperatorServerSettings {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            operator_api_token: None,
            push_public_key: None,
            origin_node_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItemCardView {
    pub id: String,
    pub source_kind: OperatorInboxSourceKind,
    pub source_kind_label: &'static str,
    pub actionable_object_id: String,
    pub workstream_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub status: OperatorInboxItemStatus,
    pub status_label: &'static str,
    pub available_actions: Vec<OperatorInboxActionKind>,
    pub available_action_labels: Vec<&'static str>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub rationale: Option<String>,
    pub provenance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxSectionView {
    pub source_kind: OperatorInboxSourceKind,
    pub title: &'static str,
    pub items: Vec<InboxItemCardView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxPageView {
    pub origin_node_id: String,
    pub actionable_count: usize,
    pub total_count: usize,
    pub sections: Vec<InboxSectionView>,
    pub empty_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationCandidateView {
    pub candidate_id: String,
    pub origin_node_id: String,
    pub item_id: String,
    pub title: String,
    pub summary: String,
    pub status: OperatorNotificationCandidateStatus,
    pub status_label: &'static str,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPageView {
    pub origin_node_id: String,
    pub candidates: Vec<NotificationCandidateView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryJobView {
    pub job_id: String,
    pub origin_node_id: String,
    pub candidate_id: String,
    pub subscription_id: String,
    pub transport_kind: String,
    pub status: NotificationDeliveryJobStatus,
    pub status_label: &'static str,
    pub summary: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryPageView {
    pub jobs: Vec<DeliveryJobView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteActionRequestView {
    pub request_id: String,
    pub origin_node_id: String,
    pub item_id: String,
    pub action_kind: OperatorInboxActionKind,
    pub action_label: &'static str,
    pub status: OperatorRemoteActionRequestStatus,
    pub status_label: &'static str,
    pub summary: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub claimed_by: Option<String>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteActionPageView {
    pub requests: Vec<RemoteActionRequestView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxDetailPageView {
    pub item: Option<InboxItemCardView>,
    pub notification_candidates: Vec<NotificationCandidateView>,
    pub delivery_jobs: Vec<DeliveryJobView>,
    pub remote_action_requests: Vec<RemoteActionRequestView>,
}

pub fn build_inbox_page(
    origin_node_id: impl Into<String>,
    items: &[OperatorInboxItem],
) -> InboxPageView {
    let origin_node_id = origin_node_id.into();
    let mut proposal_items = Vec::new();
    let mut decision_items = Vec::new();
    let mut session_items = Vec::new();
    let mut revision_items = Vec::new();
    let mut actionable_count = 0usize;

    for item in items.iter().cloned() {
        if item.status == OperatorInboxItemStatus::Open {
            actionable_count += 1;
        }
        match item.source_kind {
            OperatorInboxSourceKind::SupervisorProposal => {
                proposal_items.push(inbox_item_card_view(item))
            }
            OperatorInboxSourceKind::SupervisorDecision => {
                decision_items.push(inbox_item_card_view(item))
            }
            OperatorInboxSourceKind::PlanningSession => {
                session_items.push(inbox_item_card_view(item))
            }
            OperatorInboxSourceKind::PlanRevisionProposal => {
                revision_items.push(inbox_item_card_view(item))
            }
        }
    }

    let sections = [
        (OperatorInboxSourceKind::SupervisorProposal, proposal_items),
        (OperatorInboxSourceKind::SupervisorDecision, decision_items),
        (OperatorInboxSourceKind::PlanningSession, session_items),
        (
            OperatorInboxSourceKind::PlanRevisionProposal,
            revision_items,
        ),
    ]
    .into_iter()
    .filter_map(|(source_kind, mut items)| {
        if items.is_empty() {
            return None;
        }
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Some(InboxSectionView {
            source_kind,
            title: source_kind_label(source_kind),
            items,
        })
    })
    .collect::<Vec<_>>();

    InboxPageView {
        origin_node_id,
        actionable_count,
        total_count: items.len(),
        sections,
        empty_state: items.is_empty(),
    }
}

pub fn build_notification_page(
    origin_node_id: impl Into<String>,
    candidates: &[OperatorNotificationCandidate],
) -> NotificationPageView {
    let origin_node_id = origin_node_id.into();
    let mut candidates = candidates
        .iter()
        .cloned()
        .map(notification_candidate_view)
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.candidate_id.cmp(&b.candidate_id))
    });
    NotificationPageView {
        origin_node_id,
        candidates,
    }
}

pub fn build_delivery_page(jobs: &[NotificationDeliveryJob]) -> DeliveryPageView {
    let mut jobs = jobs
        .iter()
        .cloned()
        .map(delivery_job_view)
        .collect::<Vec<_>>();
    jobs.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.job_id.cmp(&b.job_id))
    });
    DeliveryPageView { jobs }
}

pub fn build_remote_action_page(requests: &[OperatorRemoteActionRequest]) -> RemoteActionPageView {
    let mut requests = requests
        .iter()
        .cloned()
        .map(remote_action_request_view)
        .collect::<Vec<_>>();
    requests.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.request_id.cmp(&b.request_id))
    });
    RemoteActionPageView { requests }
}

pub fn build_inbox_detail_page(
    item: Option<OperatorInboxItem>,
    candidates: &[OperatorNotificationCandidate],
    jobs: &[NotificationDeliveryJob],
    requests: &[OperatorRemoteActionRequest],
) -> InboxDetailPageView {
    let item_view = item.clone().map(inbox_item_card_view);
    let item_id = item.as_ref().map(|item| item.id.clone());
    let candidate_views = candidates
        .iter()
        .filter(|candidate| {
            item_id
                .as_deref()
                .is_none_or(|item_id| candidate.item_id == item_id)
        })
        .cloned()
        .map(notification_candidate_view)
        .collect::<Vec<_>>();
    let relevant_candidate_ids = candidate_views
        .iter()
        .map(|candidate| candidate.candidate_id.as_str())
        .collect::<Vec<_>>();
    let delivery_views = jobs
        .iter()
        .filter(|job| {
            relevant_candidate_ids.is_empty()
                || relevant_candidate_ids.contains(&job.candidate_id.as_str())
        })
        .cloned()
        .map(delivery_job_view)
        .collect::<Vec<_>>();
    let request_views = requests
        .iter()
        .filter(|request| {
            item_id
                .as_deref()
                .is_none_or(|item_id| request.item_id == item_id)
        })
        .cloned()
        .map(remote_action_request_view)
        .collect::<Vec<_>>();

    InboxDetailPageView {
        item: item_view,
        notification_candidates: candidate_views,
        delivery_jobs: delivery_views,
        remote_action_requests: request_views,
    }
}

pub fn inbox_item_card_view(item: OperatorInboxItem) -> InboxItemCardView {
    let title = item.title.clone();
    let summary = item.summary.clone();
    InboxItemCardView {
        id: item.id,
        source_kind: item.source_kind,
        source_kind_label: source_kind_label(item.source_kind),
        actionable_object_id: item.actionable_object_id,
        workstream_id: item.workstream_id,
        work_unit_id: item.work_unit_id,
        title,
        summary,
        status: item.status,
        status_label: inbox_status_label(item.status),
        available_actions: item.available_actions.clone(),
        available_action_labels: item
            .available_actions
            .iter()
            .map(|action| action_kind_label(*action))
            .collect(),
        created_at: item.created_at,
        updated_at: item.updated_at,
        resolved_at: item.resolved_at,
        rationale: item.rationale,
        provenance: item.provenance,
    }
}

pub fn notification_candidate_view(
    candidate: OperatorNotificationCandidate,
) -> NotificationCandidateView {
    NotificationCandidateView {
        candidate_id: candidate.candidate_id,
        origin_node_id: candidate.origin_node_id,
        item_id: candidate.item_id,
        title: candidate.item.title,
        summary: candidate.item.summary,
        status: candidate.status,
        status_label: notification_status_label(candidate.status),
        created_at: candidate.created_at,
        updated_at: candidate.updated_at,
    }
}

pub fn delivery_job_view(job: NotificationDeliveryJob) -> DeliveryJobView {
    let summary = job
        .error
        .as_ref()
        .map(|error| format!("error: {error}"))
        .unwrap_or_else(|| format!("{} via {}", job.candidate_id, job.subscription_id));
    DeliveryJobView {
        job_id: job.job_id,
        origin_node_id: job.origin_node_id,
        candidate_id: job.candidate_id,
        subscription_id: job.subscription_id,
        transport_kind: format!("{:?}", job.transport_kind),
        status: job.status,
        status_label: delivery_status_label(job.status),
        summary,
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

pub fn remote_action_request_view(request: OperatorRemoteActionRequest) -> RemoteActionRequestView {
    RemoteActionRequestView {
        request_id: request.request_id,
        origin_node_id: request.origin_node_id,
        item_id: request.item_id,
        action_kind: request.action_kind,
        action_label: action_kind_label(request.action_kind),
        status: request.status,
        status_label: remote_action_status_label(request.status),
        summary: request.request_note.clone().unwrap_or_else(|| {
            format!(
                "{} via {}",
                request.item.title,
                action_kind_label(request.action_kind)
            )
        }),
        created_at: request.created_at,
        updated_at: request.updated_at,
        claimed_by: request.claimed_by,
        completed_at: request.completed_at,
        failed_at: request.failed_at,
        result: request.result,
        error: request.error,
    }
}

pub fn source_kind_label(kind: OperatorInboxSourceKind) -> &'static str {
    match kind {
        OperatorInboxSourceKind::SupervisorProposal => "Supervisor proposal",
        OperatorInboxSourceKind::SupervisorDecision => "Supervisor decision",
        OperatorInboxSourceKind::PlanningSession => "Planning session",
        OperatorInboxSourceKind::PlanRevisionProposal => "Plan revision",
    }
}

pub fn inbox_status_label(status: OperatorInboxItemStatus) -> &'static str {
    match status {
        OperatorInboxItemStatus::Open => "Open",
        OperatorInboxItemStatus::Resolved => "Resolved",
        OperatorInboxItemStatus::Stale => "Stale",
        OperatorInboxItemStatus::Superseded => "Superseded",
    }
}

pub fn action_kind_label(kind: OperatorInboxActionKind) -> &'static str {
    match kind {
        OperatorInboxActionKind::Approve => "Approve",
        OperatorInboxActionKind::Reject => "Reject",
        OperatorInboxActionKind::ApproveAndSend => "Approve and send",
        OperatorInboxActionKind::RecordNoAction => "Record no action",
        OperatorInboxActionKind::ManualRefresh => "Manual refresh",
        OperatorInboxActionKind::Reconcile => "Reconcile",
        OperatorInboxActionKind::Retry => "Retry",
        OperatorInboxActionKind::Supersede => "Supersede",
        OperatorInboxActionKind::MarkReadyForReview => "Mark ready for review",
    }
}

pub fn notification_status_label(status: OperatorNotificationCandidateStatus) -> &'static str {
    match status {
        OperatorNotificationCandidateStatus::Pending => "Pending",
        OperatorNotificationCandidateStatus::Acknowledged => "Acknowledged",
        OperatorNotificationCandidateStatus::Suppressed => "Suppressed",
        OperatorNotificationCandidateStatus::Obsolete => "Obsolete",
    }
}

pub fn delivery_status_label(status: NotificationDeliveryJobStatus) -> &'static str {
    match status {
        NotificationDeliveryJobStatus::Pending => "Pending",
        NotificationDeliveryJobStatus::Dispatched => "Dispatched",
        NotificationDeliveryJobStatus::Delivered => "Delivered",
        NotificationDeliveryJobStatus::Failed => "Failed",
        NotificationDeliveryJobStatus::Suppressed => "Suppressed",
        NotificationDeliveryJobStatus::Skipped => "Skipped",
        NotificationDeliveryJobStatus::Obsolete => "Obsolete",
    }
}

pub fn remote_action_status_label(status: OperatorRemoteActionRequestStatus) -> &'static str {
    match status {
        OperatorRemoteActionRequestStatus::Pending => "Pending",
        OperatorRemoteActionRequestStatus::Claimed => "Claimed",
        OperatorRemoteActionRequestStatus::Completed => "Completed",
        OperatorRemoteActionRequestStatus::Failed => "Failed",
        OperatorRemoteActionRequestStatus::Canceled => "Canceled",
        OperatorRemoteActionRequestStatus::Stale => "Stale",
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use orcas_core::ipc::{
        NotificationDeliveryJob, NotificationDeliveryJobStatus, NotificationTransportKind,
        OperatorInboxActionKind, OperatorInboxItem, OperatorInboxItemStatus,
        OperatorInboxSourceKind, OperatorNotificationCandidate,
        OperatorNotificationCandidateStatus, OperatorRemoteActionRequest,
        OperatorRemoteActionRequestStatus,
    };

    fn inbox_item(
        id: &str,
        source_kind: OperatorInboxSourceKind,
        status: OperatorInboxItemStatus,
    ) -> OperatorInboxItem {
        let now = Utc::now();
        OperatorInboxItem {
            id: id.to_string(),
            sequence: 1,
            source_kind,
            actionable_object_id: format!("obj-{id}"),
            workstream_id: Some("ws-1".to_string()),
            work_unit_id: Some("wu-1".to_string()),
            title: format!("Title {id}"),
            summary: format!("Summary {id}"),
            status,
            available_actions: vec![
                OperatorInboxActionKind::Approve,
                OperatorInboxActionKind::Reject,
            ],
            created_at: now,
            updated_at: now,
            resolved_at: None,
            rationale: Some("why".to_string()),
            provenance: Some("test".to_string()),
        }
    }

    #[test]
    fn inbox_page_groups_actionable_items_by_source_kind() {
        let page = build_inbox_page(
            "origin-1",
            &[
                inbox_item(
                    "proposal-1",
                    OperatorInboxSourceKind::SupervisorProposal,
                    OperatorInboxItemStatus::Open,
                ),
                inbox_item(
                    "decision-1",
                    OperatorInboxSourceKind::SupervisorDecision,
                    OperatorInboxItemStatus::Resolved,
                ),
                inbox_item(
                    "session-1",
                    OperatorInboxSourceKind::PlanningSession,
                    OperatorInboxItemStatus::Open,
                ),
            ],
        );

        assert_eq!(page.origin_node_id, "origin-1");
        assert_eq!(page.actionable_count, 2);
        assert_eq!(page.total_count, 3);
        assert!(!page.empty_state);
        assert_eq!(page.sections.len(), 3);
        assert_eq!(page.sections[0].title, "Supervisor proposal");
        assert_eq!(page.sections[2].items[0].status_label, "Open");
    }

    #[test]
    fn detail_page_and_status_labels_are_stable() {
        let candidate = OperatorNotificationCandidate {
            candidate_id: "cand-1".to_string(),
            origin_node_id: "origin-1".to_string(),
            item_id: "proposal-1".to_string(),
            trigger_sequence: 1,
            status: OperatorNotificationCandidateStatus::Pending,
            item: inbox_item(
                "proposal-1",
                OperatorInboxSourceKind::SupervisorProposal,
                OperatorInboxItemStatus::Open,
            ),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            acknowledged_at: None,
            suppressed_at: None,
            resolved_at: None,
        };
        let job = NotificationDeliveryJob {
            job_id: "job-1".to_string(),
            origin_node_id: "origin-1".to_string(),
            candidate_id: "cand-1".to_string(),
            trigger_sequence: 1,
            recipient_id: "recipient-1".to_string(),
            subscription_id: "sub-1".to_string(),
            transport_kind: NotificationTransportKind::Mock,
            status: NotificationDeliveryJobStatus::Pending,
            attempt_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            dispatched_at: None,
            delivered_at: None,
            failed_at: None,
            suppressed_at: None,
            skipped_at: None,
            obsolete_at: None,
            receipt: None,
            error: None,
        };
        let request = OperatorRemoteActionRequest {
            request_id: "req-1".to_string(),
            origin_node_id: "origin-1".to_string(),
            candidate_id: "cand-1".to_string(),
            item_id: "proposal-1".to_string(),
            trigger_sequence: 1,
            action_kind: OperatorInboxActionKind::Approve,
            idempotency_key: Some("k".to_string()),
            item: inbox_item(
                "proposal-1",
                OperatorInboxSourceKind::SupervisorProposal,
                OperatorInboxItemStatus::Open,
            ),
            requested_by: Some("operator".to_string()),
            request_note: Some("note".to_string()),
            status: OperatorRemoteActionRequestStatus::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            claimed_by: None,
            claimed_at: None,
            claimed_until: None,
            claim_token: None,
            completed_at: None,
            failed_at: None,
            canceled_at: None,
            stale_at: None,
            attempt_count: 0,
            result: None,
            error: None,
        };
        let detail = build_inbox_detail_page(
            Some(candidate.item.clone()),
            &[candidate],
            &[job],
            &[request],
        );

        assert_eq!(
            notification_status_label(OperatorNotificationCandidateStatus::Pending),
            "Pending"
        );
        assert_eq!(
            delivery_status_label(NotificationDeliveryJobStatus::Pending),
            "Pending"
        );
        assert_eq!(
            remote_action_status_label(OperatorRemoteActionRequestStatus::Pending),
            "Pending"
        );
        assert_eq!(detail.notification_candidates.len(), 1);
        assert_eq!(detail.delivery_jobs.len(), 1);
        assert_eq!(detail.remote_action_requests.len(), 1);
    }
}
