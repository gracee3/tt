use chrono::Utc;

use orcas_core::{
    TrackedThreadWorkspaceOperationKind, TrackedThreadWorkspaceOperationStatus,
    WorkspaceOperationRecord,
    authority::TrackedThreadWorkspace,
    ipc::{
        TrackedThreadMergePrepAssessment, TrackedThreadMergePrepReadiness,
        TrackedThreadMergePrepReason, TrackedThreadWorkspaceInspection,
        TrackedThreadWorkspaceInspectionWarning,
    },
};

pub fn assess_merge_prep(
    workspace: &TrackedThreadWorkspace,
    inspection: Option<&TrackedThreadWorkspaceInspection>,
    operation: Option<&WorkspaceOperationRecord>,
) -> Option<TrackedThreadMergePrepAssessment> {
    let operation = operation?;
    if operation.kind != TrackedThreadWorkspaceOperationKind::MergePrep {
        return None;
    }

    let mut reasons = Vec::new();
    if operation.status != TrackedThreadWorkspaceOperationStatus::Completed
        || operation.report_id.is_none()
        || operation.report_disposition != Some(orcas_core::ReportDisposition::Completed)
    {
        reasons.push(TrackedThreadMergePrepReason::MissingSuccessfulReport);
    }

    let local_head_commit =
        inspection.and_then(|inspection| inspection.current_head_commit.clone());
    let worker_reported_head_commit = workspace.last_reported_head_commit.clone();

    if worker_reported_head_commit.is_none() {
        reasons.push(TrackedThreadMergePrepReason::MissingWorkerReportedHead);
    }

    if let Some(inspection) = inspection {
        for warning in &inspection.warnings {
            match warning {
                TrackedThreadWorkspaceInspectionWarning::MissingWorktree => {
                    reasons.push(TrackedThreadMergePrepReason::MissingWorktree);
                }
                TrackedThreadWorkspaceInspectionWarning::InvalidWorktree => {
                    reasons.push(TrackedThreadMergePrepReason::InvalidWorktree);
                }
                TrackedThreadWorkspaceInspectionWarning::DetachedHead => {
                    reasons.push(TrackedThreadMergePrepReason::DetachedHead);
                }
                TrackedThreadWorkspaceInspectionWarning::DirtyWorkspace => {
                    reasons.push(TrackedThreadMergePrepReason::DirtyWorkspace);
                }
                TrackedThreadWorkspaceInspectionWarning::BaseCommitMismatch => {
                    reasons.push(TrackedThreadMergePrepReason::BaseCommitMismatch);
                }
                TrackedThreadWorkspaceInspectionWarning::BehindLandingTarget => {
                    reasons.push(TrackedThreadMergePrepReason::BehindLandingTarget);
                }
                TrackedThreadWorkspaceInspectionWarning::DivergedFromLandingTarget => {
                    reasons.push(TrackedThreadMergePrepReason::DivergedFromLandingTarget);
                }
                TrackedThreadWorkspaceInspectionWarning::Unknown => {
                    reasons.push(TrackedThreadMergePrepReason::UnknownInspectionState);
                }
            }
        }
        if inspection.current_head_commit.is_none() {
            reasons.push(TrackedThreadMergePrepReason::UnknownInspectionState);
        }
    } else {
        reasons.push(TrackedThreadMergePrepReason::UnknownInspectionState);
    }

    if let (Some(local_head_commit), Some(worker_reported_head_commit)) = (
        local_head_commit.as_deref(),
        worker_reported_head_commit.as_deref(),
    ) && local_head_commit != worker_reported_head_commit
    {
        reasons.push(TrackedThreadMergePrepReason::HeadMismatch);
    }

    reasons.sort_by_key(|reason| *reason as u8);
    reasons.dedup();

    let readiness = if reasons.iter().any(|reason| {
        matches!(
            reason,
            TrackedThreadMergePrepReason::MissingWorktree
                | TrackedThreadMergePrepReason::InvalidWorktree
                | TrackedThreadMergePrepReason::HeadMismatch
        )
    }) {
        TrackedThreadMergePrepReadiness::Blocked
    } else if reasons.iter().any(|reason| {
        matches!(
            reason,
            TrackedThreadMergePrepReason::DirtyWorkspace
                | TrackedThreadMergePrepReason::DetachedHead
                | TrackedThreadMergePrepReason::BaseCommitMismatch
                | TrackedThreadMergePrepReason::BehindLandingTarget
                | TrackedThreadMergePrepReason::DivergedFromLandingTarget
        )
    }) {
        TrackedThreadMergePrepReadiness::NotReady
    } else if reasons.is_empty() {
        TrackedThreadMergePrepReadiness::Ready
    } else {
        TrackedThreadMergePrepReadiness::Unknown
    };

    Some(TrackedThreadMergePrepAssessment {
        assessed_at: Utc::now(),
        readiness,
        reasons,
        local_head_commit,
        worker_reported_head_commit,
        report_id: operation.report_id.clone(),
        report_disposition: operation.report_disposition,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::assess_merge_prep;
    use orcas_core::{
        ReportDisposition, TrackedThreadWorkspaceOperationKind,
        TrackedThreadWorkspaceOperationStatus, WorkspaceOperationRecord,
        authority::{
            TrackedThreadId, TrackedThreadWorkspace, TrackedThreadWorkspaceCleanupPolicy,
            TrackedThreadWorkspaceLandingPolicy, TrackedThreadWorkspaceStatus,
            TrackedThreadWorkspaceStrategy, TrackedThreadWorkspaceSyncPolicy, WorkUnitId,
        },
        ipc::{
            TrackedThreadMergePrepReadiness, TrackedThreadMergePrepReason,
            TrackedThreadWorkspaceInspection, TrackedThreadWorkspaceInspectionWarning,
        },
    };

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 20, 10, 11, 12)
            .single()
            .expect("valid timestamp")
    }

    fn sample_workspace() -> TrackedThreadWorkspace {
        TrackedThreadWorkspace {
            repository_root: "/repo".to_string(),
            owner_tracked_thread_id: TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            strategy: TrackedThreadWorkspaceStrategy::DedicatedThreadWorktree,
            worktree_path: "/repo-threads/T1".to_string(),
            branch_name: "orcas/T1".to_string(),
            base_ref: "origin/main".to_string(),
            base_commit: Some("base-123".to_string()),
            landing_target: "origin/main".to_string(),
            landing_policy: TrackedThreadWorkspaceLandingPolicy::MergeToMain,
            sync_policy: TrackedThreadWorkspaceSyncPolicy::Manual,
            cleanup_policy: TrackedThreadWorkspaceCleanupPolicy::KeepUntilCampaignClosed,
            last_reported_head_commit: Some("head-123".to_string()),
            status: TrackedThreadWorkspaceStatus::Ready,
        }
    }

    fn sample_operation() -> WorkspaceOperationRecord {
        WorkspaceOperationRecord {
            id: "assignment-1".to_string(),
            assignment_id: "assignment-1".to_string(),
            tracked_thread_id: TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            work_unit_id: WorkUnitId::parse("wu-1").expect("work unit id"),
            worker_id: Some("worker-1".to_string()),
            worker_session_id: Some("session-1".to_string()),
            kind: TrackedThreadWorkspaceOperationKind::MergePrep,
            status: TrackedThreadWorkspaceOperationStatus::Completed,
            requested_by: "supervisor_cli".to_string(),
            requested_at: fixed_now(),
            updated_at: fixed_now(),
            dispatched_at: Some(fixed_now()),
            completed_at: Some(fixed_now()),
            failed_at: None,
            canceled_at: None,
            request_note: None,
            report_id: Some("report-1".to_string()),
            report_disposition: Some(ReportDisposition::Completed),
            outcome_summary: Some("merge prep complete".to_string()),
            linked_landing_execution_id: None,
            target_worktree_path: Some("/repo-threads/T1".to_string()),
            target_branch_name: Some("orcas/T1".to_string()),
            prune_result_status: None,
            worktree_removed: None,
            branch_removed: None,
            refusal_reason: None,
            failure_reason: None,
            prune_notes: None,
        }
    }

    fn sample_inspection() -> TrackedThreadWorkspaceInspection {
        TrackedThreadWorkspaceInspection {
            inspected_at: fixed_now(),
            repository_root: "/repo".to_string(),
            worktree_path: "/repo-threads/T1".to_string(),
            exists: true,
            is_git_worktree: true,
            current_branch: Some("orcas/T1".to_string()),
            current_head_commit: Some("head-123".to_string()),
            dirty: Some(false),
            base_ref: Some("origin/main".to_string()),
            base_commit: Some("base-123".to_string()),
            landing_target: Some("origin/main".to_string()),
            base_commit_comparison: None,
            landing_target_comparison: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn reports_ready_when_report_and_inspection_align() {
        let assessment = assess_merge_prep(
            &sample_workspace(),
            Some(&sample_inspection()),
            Some(&sample_operation()),
        )
        .expect("merge prep assessment");

        assert_eq!(assessment.readiness, TrackedThreadMergePrepReadiness::Ready);
        assert!(assessment.reasons.is_empty(), "{assessment:?}");
    }

    #[test]
    fn blocks_when_heads_do_not_match() {
        let mut workspace = sample_workspace();
        workspace.last_reported_head_commit = Some("worker-head".to_string());
        let mut inspection = sample_inspection();
        inspection.current_head_commit = Some("local-head".to_string());

        let assessment =
            assess_merge_prep(&workspace, Some(&inspection), Some(&sample_operation()))
                .expect("merge prep assessment");

        assert_eq!(
            assessment.readiness,
            TrackedThreadMergePrepReadiness::Blocked
        );
        assert!(
            assessment
                .reasons
                .contains(&TrackedThreadMergePrepReason::HeadMismatch)
        );
    }

    #[test]
    fn marks_dirty_workspace_not_ready() {
        let mut inspection = sample_inspection();
        inspection
            .warnings
            .push(TrackedThreadWorkspaceInspectionWarning::DirtyWorkspace);

        let assessment = assess_merge_prep(
            &sample_workspace(),
            Some(&inspection),
            Some(&sample_operation()),
        )
        .expect("merge prep assessment");

        assert_eq!(
            assessment.readiness,
            TrackedThreadMergePrepReadiness::NotReady
        );
        assert!(
            assessment
                .reasons
                .contains(&TrackedThreadMergePrepReason::DirtyWorkspace)
        );
    }

    #[test]
    fn stays_unknown_without_successful_merge_prep_report() {
        let mut operation = sample_operation();
        operation.status = TrackedThreadWorkspaceOperationStatus::Failed;
        operation.report_disposition = Some(ReportDisposition::Blocked);

        let assessment = assess_merge_prep(
            &sample_workspace(),
            Some(&sample_inspection()),
            Some(&operation),
        )
        .expect("merge prep assessment");

        assert_eq!(
            assessment.readiness,
            TrackedThreadMergePrepReadiness::Unknown
        );
        assert!(
            assessment
                .reasons
                .contains(&TrackedThreadMergePrepReason::MissingSuccessfulReport)
        );
    }
}
