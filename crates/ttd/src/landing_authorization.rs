use tt_core::{
    LandingAuthorizationRecord, LandingAuthorizationStatus,
    authority::TrackedThreadWorkspace,
    ipc::{
        TrackedThreadMergePrepAssessment, TrackedThreadMergePrepReadiness,
        TrackedThreadWorkspaceInspection,
    },
};

pub fn landing_authorization_is_current(
    landing_authorization: Option<&LandingAuthorizationRecord>,
    workspace_inspection: Option<&TrackedThreadWorkspaceInspection>,
    merge_prep_assessment: Option<&TrackedThreadMergePrepAssessment>,
    workspace: Option<&TrackedThreadWorkspace>,
) -> Option<bool> {
    let authorization = landing_authorization?;
    let inspection = workspace_inspection?;
    let merge_prep_assessment = merge_prep_assessment?;
    let workspace = workspace?;
    Some(
        authorization.status == LandingAuthorizationStatus::Authorized
            && merge_prep_assessment.readiness == TrackedThreadMergePrepReadiness::Ready
            && inspection.current_head_commit.as_deref()
                == Some(authorization.authorized_head_commit.as_str())
            && workspace.landing_target == authorization.landing_target,
    )
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::landing_authorization_is_current;
    use tt_core::{
        LandingAuthorizationRecord, LandingAuthorizationStatus,
        authority::{
            TrackedThreadId, TrackedThreadWorkspace, TrackedThreadWorkspaceCleanupPolicy,
            TrackedThreadWorkspaceLandingPolicy, TrackedThreadWorkspaceStatus,
            TrackedThreadWorkspaceStrategy, TrackedThreadWorkspaceSyncPolicy, WorkUnitId,
        },
        ipc::{
            TrackedThreadMergePrepAssessment, TrackedThreadMergePrepReadiness,
            TrackedThreadMergePrepReason, TrackedThreadWorkspaceInspection,
        },
    };

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 20, 12, 13, 14)
            .single()
            .expect("valid timestamp")
    }

    fn sample_workspace() -> TrackedThreadWorkspace {
        TrackedThreadWorkspace {
            repository_root: "/repo".to_string(),
            owner_tracked_thread_id: TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            strategy: TrackedThreadWorkspaceStrategy::DedicatedThreadWorktree,
            worktree_path: "/repo/worktree".to_string(),
            branch_name: "tt/tt-1".to_string(),
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

    fn sample_authorization() -> LandingAuthorizationRecord {
        LandingAuthorizationRecord {
            id: "landing-auth-1".to_string(),
            tracked_thread_id: TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            work_unit_id: WorkUnitId::parse("wu-1").expect("work unit id"),
            worker_id: Some("worker-1".to_string()),
            worker_session_id: Some("session-1".to_string()),
            authorized_head_commit: "head-123".to_string(),
            landing_target: "origin/main".to_string(),
            linked_merge_prep_operation_id: "op-1".to_string(),
            merge_prep_assessed_at: fixed_now(),
            merge_prep_readiness: TrackedThreadMergePrepReadiness::Ready,
            merge_prep_reasons: Vec::new(),
            merge_prep_report_id: Some("report-1".to_string()),
            merge_prep_report_disposition: Some(tt_core::ReportDisposition::Completed),
            authorized_by: "supervisor_cli_operator".to_string(),
            authorized_at: fixed_now(),
            updated_at: fixed_now(),
            status: LandingAuthorizationStatus::Authorized,
            request_note: None,
            outcome_summary: None,
        }
    }

    fn sample_inspection() -> TrackedThreadWorkspaceInspection {
        TrackedThreadWorkspaceInspection {
            inspected_at: fixed_now(),
            repository_root: "/repo".to_string(),
            worktree_path: "/repo/worktree".to_string(),
            exists: true,
            is_git_worktree: true,
            current_branch: Some("tt/tt-1".to_string()),
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

    fn sample_assessment() -> TrackedThreadMergePrepAssessment {
        TrackedThreadMergePrepAssessment {
            assessed_at: fixed_now(),
            readiness: TrackedThreadMergePrepReadiness::Ready,
            reasons: Vec::<TrackedThreadMergePrepReason>::new(),
            local_head_commit: Some("head-123".to_string()),
            worker_reported_head_commit: Some("head-123".to_string()),
            report_id: Some("report-1".to_string()),
            report_disposition: Some(tt_core::ReportDisposition::Completed),
        }
    }

    #[test]
    fn reports_current_when_basis_matches() {
        assert_eq!(
            landing_authorization_is_current(
                Some(&sample_authorization()),
                Some(&sample_inspection()),
                Some(&sample_assessment()),
                Some(&sample_workspace()),
            ),
            Some(true)
        );
    }

    #[test]
    fn reports_not_current_when_head_changes() {
        let mut inspection = sample_inspection();
        inspection.current_head_commit = Some("head-456".to_string());

        assert_eq!(
            landing_authorization_is_current(
                Some(&sample_authorization()),
                Some(&inspection),
                Some(&sample_assessment()),
                Some(&sample_workspace()),
            ),
            Some(false)
        );
    }
}
