use orcas_core::{LandingAuthorizationRecord, LandingExecutionRecord};

pub fn landing_execution_matches_authorization_basis(
    landing_execution: Option<&LandingExecutionRecord>,
    landing_authorization: Option<&LandingAuthorizationRecord>,
) -> Option<bool> {
    let landing_execution = landing_execution?;
    let landing_authorization = landing_authorization?;
    Some(
        landing_execution.authorization_id == landing_authorization.id
            && landing_execution.authorized_head_commit
                == landing_authorization.authorized_head_commit
            && landing_execution.landing_target == landing_authorization.landing_target,
    )
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::landing_execution_matches_authorization_basis;
    use orcas_core::{
        LandingAuthorizationRecord, LandingExecutionRecord,
        TrackedThreadLandingExecutionResultStatus,
        authority::{TrackedThreadId, WorkUnitId},
    };

    fn fixed_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 20, 13, 14, 15)
            .single()
            .expect("valid timestamp")
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
            linked_merge_prep_operation_id: "merge-prep-1".to_string(),
            merge_prep_assessed_at: fixed_now(),
            merge_prep_readiness: orcas_core::ipc::TrackedThreadMergePrepReadiness::Ready,
            merge_prep_reasons: Vec::new(),
            merge_prep_report_id: Some("report-1".to_string()),
            merge_prep_report_disposition: Some(orcas_core::ReportDisposition::Completed),
            authorized_by: "supervisor_cli_operator".to_string(),
            authorized_at: fixed_now(),
            updated_at: fixed_now(),
            status: orcas_core::LandingAuthorizationStatus::Authorized,
            request_note: None,
            outcome_summary: None,
        }
    }

    fn sample_execution() -> LandingExecutionRecord {
        LandingExecutionRecord {
            id: "landing-exec-1".to_string(),
            assignment_id: "assignment-1".to_string(),
            tracked_thread_id: TrackedThreadId::parse("tt-1").expect("tracked thread id"),
            work_unit_id: WorkUnitId::parse("wu-1").expect("work unit id"),
            authorization_id: "landing-auth-1".to_string(),
            authorized_head_commit: "head-123".to_string(),
            landing_target: "origin/main".to_string(),
            worker_id: Some("worker-1".to_string()),
            worker_session_id: Some("session-1".to_string()),
            requested_by: "supervisor_cli_operator".to_string(),
            requested_at: fixed_now(),
            updated_at: fixed_now(),
            dispatched_at: Some(fixed_now()),
            completed_at: None,
            failed_at: None,
            canceled_at: None,
            request_note: None,
            report_id: None,
            report_disposition: None,
            status: orcas_core::LandingExecutionStatus::Requested,
            result_status: Some(TrackedThreadLandingExecutionResultStatus::Succeeded),
            attempted_head_commit: Some("head-123".to_string()),
            landed_commit: Some("head-456".to_string()),
            landing_ref_updated: Some(true),
            failure_reason: None,
            outcome_summary: None,
            notes: None,
        }
    }

    #[test]
    fn reports_current_when_basis_matches() {
        assert_eq!(
            landing_execution_matches_authorization_basis(
                Some(&sample_execution()),
                Some(&sample_authorization())
            ),
            Some(true)
        );
    }

    #[test]
    fn reports_not_current_when_head_changes() {
        let mut execution = sample_execution();
        execution.authorized_head_commit = "head-456".to_string();

        assert_eq!(
            landing_execution_matches_authorization_basis(
                Some(&execution),
                Some(&sample_authorization())
            ),
            Some(false)
        );
    }
}
