use serde::{Deserialize, Serialize};
use tt_operator_core::{
    DeliveryJobView, InboxItemCardView, NotificationCandidateView, RemoteActionRequestView,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceSection {
    Workstreams,
    Threads,
    Inbox,
    Notifications,
    Deliveries,
    Actions,
}

impl WorkspaceSection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workstreams => "Workstreams",
            Self::Threads => "Threads",
            Self::Inbox => "Inbox",
            Self::Notifications => "Notifications",
            Self::Deliveries => "Deliveries",
            Self::Actions => "Actions",
        }
    }

    pub const fn href(self) -> &'static str {
        match self {
            Self::Workstreams => "/workstreams",
            Self::Threads => "/threads",
            Self::Inbox => "/inbox",
            Self::Notifications => "/notifications",
            Self::Deliveries => "/deliveries",
            Self::Actions => "/actions",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceFocus {
    pub section: WorkspaceSection,
    pub kind_label: String,
    pub title: String,
    pub summary: String,
    pub status_label: String,
    pub href: String,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub candidate_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
}

impl WorkspaceFocus {
    pub fn inbox_item_placeholder(item_id: impl Into<String>) -> Self {
        let item_id = item_id.into();
        Self {
            section: WorkspaceSection::Inbox,
            kind_label: "Mirrored inbox item".to_string(),
            title: format!("Loading inbox item {item_id}"),
            summary: "Mirrored inbox item details are loading from the server.".to_string(),
            status_label: "loading".to_string(),
            href: format!("/inbox/{item_id}"),
            item_id: Some(item_id),
            request_id: None,
            candidate_id: None,
            job_id: None,
        }
    }

    pub fn notification_candidate_placeholder(
        candidate_id: impl Into<String>,
        item_id: impl Into<String>,
    ) -> Self {
        let candidate_id = candidate_id.into();
        let item_id = item_id.into();
        Self {
            section: WorkspaceSection::Notifications,
            kind_label: "Notification candidate".to_string(),
            title: format!("Loading notification candidate {candidate_id}"),
            summary: "Notification readiness details are loading from the server.".to_string(),
            status_label: "loading".to_string(),
            href: format!("/inbox/{item_id}"),
            item_id: Some(item_id),
            request_id: None,
            candidate_id: Some(candidate_id),
            job_id: None,
        }
    }

    pub fn delivery_job_placeholder(
        job_id: impl Into<String>,
        candidate_id: impl Into<String>,
    ) -> Self {
        let job_id = job_id.into();
        let candidate_id = candidate_id.into();
        Self {
            section: WorkspaceSection::Deliveries,
            kind_label: "Delivery job".to_string(),
            title: format!("Loading delivery job {job_id}"),
            summary: "Delivery job details are loading from the server.".to_string(),
            status_label: "loading".to_string(),
            href: "/deliveries".to_string(),
            item_id: None,
            request_id: None,
            candidate_id: Some(candidate_id),
            job_id: Some(job_id),
        }
    }

    pub fn remote_action_request_placeholder(request_id: impl Into<String>) -> Self {
        let request_id = request_id.into();
        Self {
            section: WorkspaceSection::Actions,
            kind_label: "Remote action request".to_string(),
            title: format!("Loading remote action request {request_id}"),
            summary: "Remote action request details are loading from the server.".to_string(),
            status_label: "loading".to_string(),
            href: format!("/actions/{request_id}"),
            item_id: None,
            request_id: Some(request_id),
            candidate_id: None,
            job_id: None,
        }
    }

    pub fn from_inbox_item(item: &InboxItemCardView) -> Self {
        Self {
            section: WorkspaceSection::Inbox,
            kind_label: "Mirrored inbox item".to_string(),
            title: item.title.clone(),
            summary: item.summary.clone(),
            status_label: item.status_label.to_string(),
            href: format!("/inbox/{}", item.id),
            item_id: Some(item.id.clone()),
            request_id: None,
            candidate_id: None,
            job_id: None,
        }
    }

    pub fn from_notification_candidate(candidate: &NotificationCandidateView) -> Self {
        Self {
            section: WorkspaceSection::Notifications,
            kind_label: "Notification candidate".to_string(),
            title: candidate.title.clone(),
            summary: candidate.summary.clone(),
            status_label: candidate.status_label.to_string(),
            href: format!("/inbox/{}", candidate.item_id),
            item_id: Some(candidate.item_id.clone()),
            request_id: None,
            candidate_id: Some(candidate.candidate_id.clone()),
            job_id: None,
        }
    }

    pub fn from_delivery_job(job: &DeliveryJobView) -> Self {
        Self {
            section: WorkspaceSection::Deliveries,
            kind_label: "Delivery job".to_string(),
            title: job.summary.clone(),
            summary: format!(
                "Delivery job {} on transport {}",
                job.job_id, job.transport_kind
            ),
            status_label: job.status_label.to_string(),
            href: "/deliveries".to_string(),
            item_id: None,
            request_id: None,
            candidate_id: Some(job.candidate_id.clone()),
            job_id: Some(job.job_id.clone()),
        }
    }

    pub fn from_remote_action_request(request: &RemoteActionRequestView) -> Self {
        Self {
            section: WorkspaceSection::Actions,
            kind_label: "Remote action request".to_string(),
            title: request.summary.clone(),
            summary: format!("Remote action request {}", request.request_id),
            status_label: request.status_label.to_string(),
            href: format!("/actions/{}", request.request_id),
            item_id: Some(request.item_id.clone()),
            request_id: Some(request.request_id.clone()),
            candidate_id: None,
            job_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub active_section: WorkspaceSection,
    #[serde(default)]
    pub focus: Option<WorkspaceFocus>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            active_section: WorkspaceSection::Workstreams,
            focus: None,
        }
    }
}

impl WorkspaceState {
    pub fn focus_item_id(&self) -> Option<&str> {
        self.focus.as_ref()?.item_id.as_deref()
    }

    pub fn focus_request_id(&self) -> Option<&str> {
        self.focus.as_ref()?.request_id.as_deref()
    }

    pub fn focus_candidate_id(&self) -> Option<&str> {
        self.focus.as_ref()?.candidate_id.as_deref()
    }

    pub fn focus_job_id(&self) -> Option<&str> {
        self.focus.as_ref()?.job_id.as_deref()
    }

    pub fn focus_matches_inbox_item(&self, item_id: &str) -> bool {
        self.focus_item_id() == Some(item_id)
    }

    pub fn focus_matches_notification_candidate(&self, candidate_id: &str, item_id: &str) -> bool {
        self.focus_candidate_id() == Some(candidate_id) || self.focus_item_id() == Some(item_id)
    }

    pub fn focus_matches_delivery_job(&self, job_id: &str, candidate_id: &str) -> bool {
        self.focus_job_id() == Some(job_id) || self.focus_candidate_id() == Some(candidate_id)
    }

    pub fn focus_matches_remote_action_request(&self, request_id: &str) -> bool {
        self.focus_request_id() == Some(request_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tt_core::ipc::{
        NotificationDeliveryJobStatus, OperatorInboxActionKind, OperatorInboxItemStatus,
        OperatorInboxSourceKind, OperatorNotificationCandidateStatus,
        OperatorRemoteActionRequestStatus,
    };

    fn inbox_item() -> InboxItemCardView {
        let now = Utc::now();
        InboxItemCardView {
            id: "item-1".to_string(),
            source_kind: OperatorInboxSourceKind::SupervisorProposal,
            source_kind_label: "proposal",
            actionable_object_id: "object-1".to_string(),
            workstream_id: Some("workstream-1".to_string()),
            work_unit_id: Some("work-unit-1".to_string()),
            title: "Review me".to_string(),
            summary: "A mirrored inbox item".to_string(),
            status: OperatorInboxItemStatus::Open,
            status_label: "open",
            available_actions: vec![OperatorInboxActionKind::Approve],
            available_action_labels: vec!["approve"],
            created_at: now,
            updated_at: now,
            resolved_at: None,
            rationale: None,
            provenance: None,
        }
    }

    fn notification_candidate() -> NotificationCandidateView {
        let now = Utc::now();
        NotificationCandidateView {
            candidate_id: "candidate-1".to_string(),
            origin_node_id: "origin-a".to_string(),
            item_id: "item-1".to_string(),
            title: "Review me".to_string(),
            summary: "Notification candidate".to_string(),
            status: OperatorNotificationCandidateStatus::Pending,
            status_label: "pending",
            created_at: now,
            updated_at: now,
        }
    }

    fn delivery_job() -> DeliveryJobView {
        let now = Utc::now();
        DeliveryJobView {
            job_id: "job-1".to_string(),
            origin_node_id: "origin-a".to_string(),
            candidate_id: "candidate-1".to_string(),
            subscription_id: "subscription-1".to_string(),
            transport_kind: "mock".to_string(),
            status: NotificationDeliveryJobStatus::Pending,
            status_label: "pending",
            summary: "Delivery job".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    fn remote_action_request() -> RemoteActionRequestView {
        let now = Utc::now();
        RemoteActionRequestView {
            request_id: "request-1".to_string(),
            origin_node_id: "origin-a".to_string(),
            item_id: "item-1".to_string(),
            action_kind: OperatorInboxActionKind::Approve,
            action_label: "Approve",
            status: OperatorRemoteActionRequestStatus::Pending,
            status_label: "pending",
            summary: "Remote action request".to_string(),
            created_at: now,
            updated_at: now,
            claimed_by: None,
            completed_at: None,
            failed_at: None,
            result: None,
            error: None,
        }
    }

    #[test]
    fn workspace_section_labels_and_hrefs_are_stable() {
        assert_eq!(WorkspaceSection::Inbox.label(), "Inbox");
        assert_eq!(WorkspaceSection::Threads.href(), "/threads");
        assert_eq!(WorkspaceSection::Notifications.href(), "/notifications");
        assert_eq!(WorkspaceSection::Deliveries.label(), "Deliveries");
        assert_eq!(WorkspaceSection::Actions.href(), "/actions");
    }

    #[test]
    fn workspace_state_defaults_to_workstreams_section() {
        let state = WorkspaceState::default();
        assert_eq!(state.active_section, WorkspaceSection::Workstreams);
        assert!(state.focus.is_none());
    }

    #[test]
    fn workspace_focus_helpers_preserve_related_ids() {
        let state = WorkspaceState {
            active_section: WorkspaceSection::Inbox,
            focus: Some(WorkspaceFocus::from_remote_action_request(
                &remote_action_request(),
            )),
        };
        assert_eq!(state.focus_item_id(), Some("item-1"));
        assert_eq!(state.focus_request_id(), Some("request-1"));
        assert!(state.focus_matches_remote_action_request("request-1"));

        let notification_focus =
            WorkspaceFocus::from_notification_candidate(&notification_candidate());
        assert_eq!(
            notification_focus.candidate_id.as_deref(),
            Some("candidate-1")
        );
        assert_eq!(notification_focus.href, "/inbox/item-1");
        let notification_state = WorkspaceState {
            active_section: WorkspaceSection::Notifications,
            focus: Some(notification_focus),
        };
        assert!(notification_state.focus_matches_notification_candidate("candidate-1", "item-1"));

        let delivery_focus = WorkspaceFocus::from_delivery_job(&delivery_job());
        assert_eq!(delivery_focus.job_id.as_deref(), Some("job-1"));
        assert_eq!(delivery_focus.href, "/deliveries");
        let delivery_state = WorkspaceState {
            active_section: WorkspaceSection::Deliveries,
            focus: Some(delivery_focus),
        };
        assert!(delivery_state.focus_matches_delivery_job("job-1", "candidate-1"));

        let inbox_focus = WorkspaceFocus::from_inbox_item(&inbox_item());
        assert_eq!(inbox_focus.item_id.as_deref(), Some("item-1"));
        assert_eq!(inbox_focus.href, "/inbox/item-1");
        let inbox_state = WorkspaceState {
            active_section: WorkspaceSection::Inbox,
            focus: Some(inbox_focus),
        };
        assert!(inbox_state.focus_matches_inbox_item("item-1"));
    }
}
