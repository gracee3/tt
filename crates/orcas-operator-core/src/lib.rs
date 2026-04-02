use chrono::{DateTime, Utc};
use orcas_core::ipc::{
    NotificationDeliveryJob, NotificationDeliveryJobStatus, OperatorInboxActionKind,
    OperatorInboxItem, OperatorInboxItemStatus, OperatorInboxSourceKind,
    OperatorNotificationCandidate, OperatorNotificationCandidateStatus,
    OperatorRemoteActionRequest, OperatorRemoteActionRequestStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewChangeSummary {
    pub headline: String,
    pub detail: String,
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

fn index_inbox_items<'a>(page: &'a InboxPageView) -> BTreeMap<&'a str, &'a InboxItemCardView> {
    page.sections
        .iter()
        .flat_map(|section| section.items.iter())
        .map(|item| (item.id.as_str(), item))
        .collect()
}

fn index_notification_candidates<'a>(
    page: &'a NotificationPageView,
) -> BTreeMap<&'a str, &'a NotificationCandidateView> {
    page.candidates
        .iter()
        .map(|candidate| (candidate.candidate_id.as_str(), candidate))
        .collect()
}

fn index_delivery_jobs<'a>(page: &'a DeliveryPageView) -> BTreeMap<&'a str, &'a DeliveryJobView> {
    page.jobs
        .iter()
        .map(|job| (job.job_id.as_str(), job))
        .collect()
}

fn index_remote_action_requests<'a>(
    page: &'a RemoteActionPageView,
) -> BTreeMap<&'a str, &'a RemoteActionRequestView> {
    page.requests
        .iter()
        .map(|request| (request.request_id.as_str(), request))
        .collect()
}

fn first_changed_inbox_item<'a>(
    previous: &BTreeMap<&'a str, &'a InboxItemCardView>,
    current: &'a InboxPageView,
) -> Option<ViewChangeSummary> {
    for item in current
        .sections
        .iter()
        .flat_map(|section| section.items.iter())
    {
        let Some(previous_item) = previous.get(item.id.as_str()) else {
            return Some(ViewChangeSummary {
                headline: "New mirrored inbox item".to_string(),
                detail: format!(
                    "{} is now mirrored on the server with {} status.",
                    item.title, item.status_label
                ),
            });
        };

        if previous_item.status != item.status {
            return Some(ViewChangeSummary {
                headline: "Mirrored inbox status changed".to_string(),
                detail: format!(
                    "{} moved from {} to {}.",
                    item.title, previous_item.status_label, item.status_label
                ),
            });
        }

        if previous_item.title != item.title
            || previous_item.summary != item.summary
            || previous_item.updated_at != item.updated_at
            || previous_item.resolved_at != item.resolved_at
        {
            return Some(ViewChangeSummary {
                headline: "Mirrored inbox item updated".to_string(),
                detail: format!("{} was updated on the server.", item.title),
            });
        }
    }

    None
}

fn first_removed_inbox_item<'a>(
    previous: &BTreeMap<&'a str, &'a InboxItemCardView>,
    current: &'a InboxPageView,
) -> Option<ViewChangeSummary> {
    let current_ids = index_inbox_items(current);
    previous
        .values()
        .find(|item| !current_ids.contains_key(item.id.as_str()))
        .map(|item| ViewChangeSummary {
            headline: "Mirrored inbox item removed".to_string(),
            detail: format!("{} is no longer present in the mirrored inbox.", item.title),
        })
}

pub fn summarize_inbox_page_change(
    previous: Option<&InboxPageView>,
    current: &InboxPageView,
) -> Option<ViewChangeSummary> {
    let previous = previous?;
    let previous_index = index_inbox_items(previous);
    if let Some(change) = first_changed_inbox_item(&previous_index, current) {
        return Some(change);
    }
    if let Some(change) = first_removed_inbox_item(&previous_index, current) {
        return Some(change);
    }
    if previous.actionable_count != current.actionable_count
        || previous.total_count != current.total_count
    {
        return Some(ViewChangeSummary {
            headline: "Mirrored inbox counts changed".to_string(),
            detail: format!(
                "{} actionable / {} total items now, was {} actionable / {} total items.",
                current.actionable_count,
                current.total_count,
                previous.actionable_count,
                previous.total_count
            ),
        });
    }
    None
}

fn first_changed_notification_candidate<'a>(
    previous: &BTreeMap<&'a str, &'a NotificationCandidateView>,
    current: &'a NotificationPageView,
) -> Option<ViewChangeSummary> {
    for candidate in &current.candidates {
        let Some(previous_candidate) = previous.get(candidate.candidate_id.as_str()) else {
            return Some(ViewChangeSummary {
                headline: "New notification candidate".to_string(),
                detail: format!("{} is now pending for mirrored review.", candidate.title),
            });
        };

        if previous_candidate.status != candidate.status {
            return Some(ViewChangeSummary {
                headline: "Notification candidate status changed".to_string(),
                detail: format!(
                    "{} moved from {} to {}.",
                    candidate.title, previous_candidate.status_label, candidate.status_label
                ),
            });
        }

        if previous_candidate.title != candidate.title
            || previous_candidate.summary != candidate.summary
            || previous_candidate.updated_at != candidate.updated_at
        {
            return Some(ViewChangeSummary {
                headline: "Notification candidate updated".to_string(),
                detail: format!("{} was updated on the server.", candidate.title),
            });
        }
    }

    None
}

fn first_removed_notification_candidate<'a>(
    previous: &BTreeMap<&'a str, &'a NotificationCandidateView>,
    current: &'a NotificationPageView,
) -> Option<ViewChangeSummary> {
    let current_ids = index_notification_candidates(current);
    previous
        .values()
        .find(|candidate| !current_ids.contains_key(candidate.candidate_id.as_str()))
        .map(|candidate| ViewChangeSummary {
            headline: "Notification candidate removed".to_string(),
            detail: format!(
                "{} is no longer present in notification readiness.",
                candidate.title
            ),
        })
}

pub fn summarize_notification_page_change(
    previous: Option<&NotificationPageView>,
    current: &NotificationPageView,
) -> Option<ViewChangeSummary> {
    let previous = previous?;
    let previous_index = index_notification_candidates(previous);
    if let Some(change) = first_changed_notification_candidate(&previous_index, current) {
        return Some(change);
    }
    if let Some(change) = first_removed_notification_candidate(&previous_index, current) {
        return Some(change);
    }
    if previous.candidates.len() != current.candidates.len() {
        return Some(ViewChangeSummary {
            headline: "Notification candidates changed".to_string(),
            detail: format!(
                "{} candidates now mirrored, was {}.",
                current.candidates.len(),
                previous.candidates.len()
            ),
        });
    }
    None
}

fn first_changed_delivery_job<'a>(
    previous: &BTreeMap<&'a str, &'a DeliveryJobView>,
    current: &'a DeliveryPageView,
) -> Option<ViewChangeSummary> {
    for job in &current.jobs {
        let Some(previous_job) = previous.get(job.job_id.as_str()) else {
            return Some(ViewChangeSummary {
                headline: "New delivery job".to_string(),
                detail: format!("{} is now tracked for delivery.", job.summary),
            });
        };

        if previous_job.status != job.status {
            return Some(ViewChangeSummary {
                headline: "Delivery job status changed".to_string(),
                detail: format!(
                    "{} moved from {} to {}.",
                    job.summary, previous_job.status_label, job.status_label
                ),
            });
        }

        if previous_job.summary != job.summary
            || previous_job.updated_at != job.updated_at
            || previous_job.transport_kind != job.transport_kind
        {
            return Some(ViewChangeSummary {
                headline: "Delivery job updated".to_string(),
                detail: format!("{} was updated on the server.", job.summary),
            });
        }
    }

    None
}

fn first_removed_delivery_job<'a>(
    previous: &BTreeMap<&'a str, &'a DeliveryJobView>,
    current: &'a DeliveryPageView,
) -> Option<ViewChangeSummary> {
    let current_ids = index_delivery_jobs(current);
    previous
        .values()
        .find(|job| !current_ids.contains_key(job.job_id.as_str()))
        .map(|job| ViewChangeSummary {
            headline: "Delivery job removed".to_string(),
            detail: format!("{} is no longer present in delivery state.", job.summary),
        })
}

pub fn summarize_delivery_page_change(
    previous: Option<&DeliveryPageView>,
    current: &DeliveryPageView,
) -> Option<ViewChangeSummary> {
    let previous = previous?;
    let previous_index = index_delivery_jobs(previous);
    if let Some(change) = first_changed_delivery_job(&previous_index, current) {
        return Some(change);
    }
    if let Some(change) = first_removed_delivery_job(&previous_index, current) {
        return Some(change);
    }
    if previous.jobs.len() != current.jobs.len() {
        return Some(ViewChangeSummary {
            headline: "Delivery jobs changed".to_string(),
            detail: format!(
                "{} delivery jobs now mirrored, was {}.",
                current.jobs.len(),
                previous.jobs.len()
            ),
        });
    }
    None
}

fn first_changed_remote_action_request<'a>(
    previous: &BTreeMap<&'a str, &'a RemoteActionRequestView>,
    current: &'a RemoteActionPageView,
) -> Option<ViewChangeSummary> {
    for request in &current.requests {
        let Some(previous_request) = previous.get(request.request_id.as_str()) else {
            return Some(ViewChangeSummary {
                headline: "New remote action request".to_string(),
                detail: format!("{} is now tracked by the server.", request.summary),
            });
        };

        if previous_request.status != request.status {
            return Some(ViewChangeSummary {
                headline: "Remote action status changed".to_string(),
                detail: format!(
                    "{} moved from {} to {}.",
                    request.summary, previous_request.status_label, request.status_label
                ),
            });
        }

        if previous_request.claimed_by != request.claimed_by
            || previous_request.completed_at != request.completed_at
            || previous_request.failed_at != request.failed_at
            || previous_request.result != request.result
            || previous_request.error != request.error
            || previous_request.updated_at != request.updated_at
        {
            let detail = if matches!(request.status, OperatorRemoteActionRequestStatus::Completed) {
                "The request completed and the result was updated on the server.".to_string()
            } else if matches!(request.status, OperatorRemoteActionRequestStatus::Failed) {
                request.error.clone().unwrap_or_else(|| {
                    "The request failed and the failure details were updated on the server."
                        .to_string()
                })
            } else {
                format!("{} was updated on the server.", request.summary)
            };
            return Some(ViewChangeSummary {
                headline: "Remote action request updated".to_string(),
                detail,
            });
        }
    }

    None
}

fn first_removed_remote_action_request<'a>(
    previous: &BTreeMap<&'a str, &'a RemoteActionRequestView>,
    current: &'a RemoteActionPageView,
) -> Option<ViewChangeSummary> {
    let current_ids = index_remote_action_requests(current);
    previous
        .values()
        .find(|request| !current_ids.contains_key(request.request_id.as_str()))
        .map(|request| ViewChangeSummary {
            headline: "Remote action request removed".to_string(),
            detail: format!(
                "{} is no longer present in remote action state.",
                request.summary
            ),
        })
}

pub fn summarize_remote_action_page_change(
    previous: Option<&RemoteActionPageView>,
    current: &RemoteActionPageView,
) -> Option<ViewChangeSummary> {
    let previous = previous?;
    let previous_index = index_remote_action_requests(previous);
    if let Some(change) = first_changed_remote_action_request(&previous_index, current) {
        return Some(change);
    }
    if let Some(change) = first_removed_remote_action_request(&previous_index, current) {
        return Some(change);
    }
    if previous.requests.len() != current.requests.len() {
        return Some(ViewChangeSummary {
            headline: "Remote action requests changed".to_string(),
            detail: format!(
                "{} requests now mirrored, was {}.",
                current.requests.len(),
                previous.requests.len()
            ),
        });
    }
    None
}

pub fn summarize_remote_action_request_change(
    previous: Option<&RemoteActionRequestView>,
    current: &RemoteActionRequestView,
) -> Option<ViewChangeSummary> {
    let previous = previous?;
    if previous.status != current.status {
        return Some(ViewChangeSummary {
            headline: "Remote action status changed".to_string(),
            detail: format!(
                "{} moved from {} to {}.",
                current.summary, previous.status_label, current.status_label
            ),
        });
    }
    if previous.claimed_by != current.claimed_by
        || previous.completed_at != current.completed_at
        || previous.failed_at != current.failed_at
        || previous.result != current.result
        || previous.error != current.error
        || previous.updated_at != current.updated_at
    {
        let detail = if matches!(current.status, OperatorRemoteActionRequestStatus::Completed) {
            "The request completed and the result was updated on the server.".to_string()
        } else if matches!(current.status, OperatorRemoteActionRequestStatus::Failed) {
            current.error.clone().unwrap_or_else(|| {
                "The request failed and the failure details were updated on the server.".to_string()
            })
        } else {
            format!("{} was updated on the server.", current.summary)
        };
        return Some(ViewChangeSummary {
            headline: "Remote action request updated".to_string(),
            detail,
        });
    }
    None
}

pub fn pending_remote_action_request_for_item_action<'a>(
    requests: &'a [RemoteActionRequestView],
    item_id: &str,
    action_kind: OperatorInboxActionKind,
) -> Option<&'a RemoteActionRequestView> {
    requests.iter().find(|request| {
        request.item_id == item_id
            && request.action_kind == action_kind
            && matches!(
                request.status,
                OperatorRemoteActionRequestStatus::Pending
                    | OperatorRemoteActionRequestStatus::Claimed
            )
    })
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

pub fn inbox_status_hint(status: OperatorInboxItemStatus) -> &'static str {
    match status {
        OperatorInboxItemStatus::Open => "Ready for operator review.",
        OperatorInboxItemStatus::Resolved => "Already resolved on the server.",
        OperatorInboxItemStatus::Stale => "The source record is no longer current.",
        OperatorInboxItemStatus::Superseded => "A newer source record replaced this item.",
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

pub fn notification_status_hint(status: OperatorNotificationCandidateStatus) -> &'static str {
    match status {
        OperatorNotificationCandidateStatus::Pending => "Eligible for delivery or operator review.",
        OperatorNotificationCandidateStatus::Acknowledged => {
            "Seen by an operator or client already."
        }
        OperatorNotificationCandidateStatus::Suppressed => "Hidden by operator choice.",
        OperatorNotificationCandidateStatus::Obsolete => "No longer relevant on the server.",
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

pub fn delivery_status_hint(status: NotificationDeliveryJobStatus) -> &'static str {
    match status {
        NotificationDeliveryJobStatus::Pending => "Waiting to be dispatched.",
        NotificationDeliveryJobStatus::Dispatched => "Handed to a transport adapter.",
        NotificationDeliveryJobStatus::Delivered => "Transport reported success.",
        NotificationDeliveryJobStatus::Failed => "Transport reported a failure.",
        NotificationDeliveryJobStatus::Suppressed => "Delivery was intentionally suppressed.",
        NotificationDeliveryJobStatus::Skipped => "No delivery was needed.",
        NotificationDeliveryJobStatus::Obsolete => "The candidate became obsolete first.",
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

pub fn remote_action_status_hint(status: OperatorRemoteActionRequestStatus) -> &'static str {
    match status {
        OperatorRemoteActionRequestStatus::Pending => "Awaiting claim or execution.",
        OperatorRemoteActionRequestStatus::Claimed => "Claimed by a daemon worker.",
        OperatorRemoteActionRequestStatus::Completed => "Executed successfully on the daemon.",
        OperatorRemoteActionRequestStatus::Failed => "Executed, but the daemon reported failure.",
        OperatorRemoteActionRequestStatus::Canceled => "Canceled before completion.",
        OperatorRemoteActionRequestStatus::Stale => "The request is no longer current.",
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

    #[test]
    fn inbox_page_change_summary_tracks_status_changes_and_clears_on_repeat() {
        let previous = build_inbox_page(
            "origin-1",
            &[inbox_item(
                "proposal-1",
                OperatorInboxSourceKind::SupervisorProposal,
                OperatorInboxItemStatus::Open,
            )],
        );
        let current = build_inbox_page(
            "origin-1",
            &[inbox_item(
                "proposal-1",
                OperatorInboxSourceKind::SupervisorProposal,
                OperatorInboxItemStatus::Resolved,
            )],
        );

        let first = summarize_inbox_page_change(Some(&previous), &current)
            .expect("status transition summary");
        assert_eq!(first.headline, "Mirrored inbox status changed");
        assert!(first.detail.contains("moved from Open to Resolved"));
        assert!(summarize_inbox_page_change(Some(&current), &current).is_none());
    }

    #[test]
    fn notification_page_change_summary_detects_pending_to_obsolete_transitions() {
        let now = Utc::now();
        let previous = NotificationPageView {
            origin_node_id: "origin-1".to_string(),
            candidates: vec![NotificationCandidateView {
                candidate_id: "candidate-1".to_string(),
                origin_node_id: "origin-1".to_string(),
                item_id: "proposal-1".to_string(),
                title: "Review proposal".to_string(),
                summary: "Please review".to_string(),
                status: OperatorNotificationCandidateStatus::Pending,
                status_label: "Pending",
                created_at: now,
                updated_at: now,
            }],
        };
        let current = NotificationPageView {
            candidates: vec![NotificationCandidateView {
                status: OperatorNotificationCandidateStatus::Obsolete,
                status_label: "Obsolete",
                updated_at: Utc::now(),
                ..previous.candidates[0].clone()
            }],
            ..previous.clone()
        };

        let summary = summarize_notification_page_change(Some(&previous), &current)
            .expect("notification transition summary");
        assert_eq!(summary.headline, "Notification candidate status changed");
        assert!(summary.detail.contains("Pending to Obsolete"));
        assert!(summarize_notification_page_change(Some(&current), &current).is_none());
    }

    #[test]
    fn delivery_page_change_summary_tracks_terminal_state_changes() {
        let now = Utc::now();
        let previous = DeliveryPageView {
            jobs: vec![DeliveryJobView {
                job_id: "job-1".to_string(),
                origin_node_id: "origin-1".to_string(),
                candidate_id: "candidate-1".to_string(),
                subscription_id: "sub-1".to_string(),
                transport_kind: "Mock".to_string(),
                status: NotificationDeliveryJobStatus::Pending,
                status_label: "Pending",
                summary: "candidate-1 via sub-1".to_string(),
                created_at: now,
                updated_at: now,
            }],
        };
        let current = DeliveryPageView {
            jobs: vec![DeliveryJobView {
                status: NotificationDeliveryJobStatus::Failed,
                status_label: "Failed",
                summary: "error: transport unavailable".to_string(),
                updated_at: Utc::now(),
                ..previous.jobs[0].clone()
            }],
        };
        let summary =
            summarize_delivery_page_change(Some(&previous), &current).expect("delivery summary");
        assert_eq!(summary.headline, "Delivery job status changed");
        assert!(summary.detail.contains("Pending to Failed"));
        assert!(summarize_delivery_page_change(Some(&current), &current).is_none());
    }

    #[test]
    fn remote_action_request_change_summary_emphasizes_terminal_outcomes() {
        let now = Utc::now();
        let previous = RemoteActionRequestView {
            request_id: "request-1".to_string(),
            origin_node_id: "origin-1".to_string(),
            item_id: "proposal-1".to_string(),
            action_kind: OperatorInboxActionKind::Approve,
            action_label: "Approve",
            status: OperatorRemoteActionRequestStatus::Claimed,
            status_label: "Claimed",
            summary: "Review proposal".to_string(),
            created_at: now,
            updated_at: now,
            claimed_by: Some("daemon-1".to_string()),
            completed_at: None,
            failed_at: None,
            result: None,
            error: None,
        };
        let current = RemoteActionRequestView {
            status: OperatorRemoteActionRequestStatus::Completed,
            status_label: "Completed",
            completed_at: Some(Utc::now()),
            result: Some(serde_json::json!({"ok": true})),
            ..previous.clone()
        };
        let summary = summarize_remote_action_request_change(Some(&previous), &current)
            .expect("remote action summary");
        assert_eq!(summary.headline, "Remote action status changed");
        assert!(summary.detail.contains("Claimed to Completed"));
        assert!(summarize_remote_action_request_change(Some(&current), &current).is_none());
    }

    #[test]
    fn remote_action_request_change_summary_uses_failure_details() {
        let now = Utc::now();
        let previous = RemoteActionRequestView {
            request_id: "request-1".to_string(),
            origin_node_id: "origin-1".to_string(),
            item_id: "proposal-1".to_string(),
            action_kind: OperatorInboxActionKind::Approve,
            action_label: "Approve",
            status: OperatorRemoteActionRequestStatus::Claimed,
            status_label: "Claimed",
            summary: "Review proposal".to_string(),
            created_at: now,
            updated_at: now,
            claimed_by: Some("daemon-1".to_string()),
            completed_at: None,
            failed_at: None,
            result: None,
            error: None,
        };
        let current = RemoteActionRequestView {
            status: OperatorRemoteActionRequestStatus::Failed,
            status_label: "Failed",
            failed_at: Some(Utc::now()),
            error: Some("daemon reported transport failure".to_string()),
            ..previous.clone()
        };
        let summary = summarize_remote_action_request_change(Some(&previous), &current)
            .expect("remote action failure summary");
        assert_eq!(summary.headline, "Remote action status changed");
        assert!(summary.detail.contains("Claimed to Failed"));
        assert!(summarize_remote_action_request_change(Some(&current), &current).is_none());
    }

    #[test]
    fn pending_remote_action_request_lookup_matches_item_and_action() {
        let now = Utc::now();
        let request = RemoteActionRequestView {
            request_id: "request-1".to_string(),
            origin_node_id: "origin-1".to_string(),
            item_id: "proposal-1".to_string(),
            action_kind: OperatorInboxActionKind::Approve,
            action_label: "Approve",
            status: OperatorRemoteActionRequestStatus::Pending,
            status_label: "Pending",
            summary: "Review proposal".to_string(),
            created_at: now,
            updated_at: now,
            claimed_by: None,
            completed_at: None,
            failed_at: None,
            result: None,
            error: None,
        };

        let found = pending_remote_action_request_for_item_action(
            std::slice::from_ref(&request),
            "proposal-1",
            OperatorInboxActionKind::Approve,
        );
        assert_eq!(
            found.map(|request| request.request_id.as_str()),
            Some("request-1")
        );

        let missing = pending_remote_action_request_for_item_action(
            std::slice::from_ref(&request),
            "proposal-1",
            OperatorInboxActionKind::Reject,
        );
        assert!(missing.is_none());
    }

    #[test]
    fn status_hints_cover_terminal_and_obsolete_states() {
        assert_eq!(
            inbox_status_hint(OperatorInboxItemStatus::Superseded),
            "A newer source record replaced this item."
        );
        assert_eq!(
            notification_status_hint(OperatorNotificationCandidateStatus::Obsolete),
            "No longer relevant on the server."
        );
        assert_eq!(
            delivery_status_hint(NotificationDeliveryJobStatus::Failed),
            "Transport reported a failure."
        );
        assert_eq!(
            remote_action_status_hint(OperatorRemoteActionRequestStatus::Failed),
            "Executed, but the daemon reported failure."
        );
    }
}
