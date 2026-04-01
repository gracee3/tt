#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code, unused_variables))]

use orcas_core::authority;
use orcas_core::ipc::{
    AssignmentStartRequest, AssignmentStartResponse,
    AuthorityDeletePlanRequest, AuthorityHierarchyGetRequest,
    AuthorityTrackedThreadCreateRequest, AuthorityTrackedThreadDeleteRequest,
    AuthorityTrackedThreadEditRequest, ThreadGetRequest, ThreadGetResponse,
    CodexAssignmentPauseRequest, CodexAssignmentResumeRequest,
    AuthorityWorkstreamCreateRequest, AuthorityWorkstreamEditRequest,
    AuthorityWorkunitCreateRequest, AuthorityWorkunitDeleteRequest,
    AuthorityWorkunitEditRequest, AuthorityWorkunitGetRequest, StateGetRequest,
    NotificationDeliveryJobListRequest, NotificationRecipientUpsertRequest,
    NotificationSubscriptionListRequest, NotificationSubscriptionSetEnabledRequest,
    NotificationSubscriptionUpsertRequest, NotificationTransportKind,
    OperatorInboxWaitForCheckpointRequest, OperatorInboxWaitForCheckpointResponse,
    OperatorNotificationListRequest, OperatorReadModelCheckpointQueryRequest,
    OperatorReadModelWaitForCheckpointRequest, OperatorRemoteActionCreateRequest,
    OperatorRemoteActionGetRequest, OperatorRemoteActionListRequest,
    OperatorRemoteActionWaitRequest, ProposalCreateRequest, ProposalCreateResponse,
};
use orcas_operator_core::{
    DeliveryPageView, InboxDetailPageView, InboxPageView, NotificationPageView,
    OperatorServerSettings, RemoteActionPageView, build_delivery_page, build_inbox_detail_page,
    build_inbox_page, build_notification_page, build_remote_action_page,
};
use orcas_server_client::OrcasServerClient;
use uuid::Uuid;

use crate::pwa::{
    self, BrowserNotificationPermission, BrowserPushState, BrowserPushSubscriptionSnapshot,
};
use crate::storage;
use crate::workstreams::WorkstreamsDashboardData;

fn client_from_settings(settings: &OperatorServerSettings) -> Result<OrcasServerClient, String> {
    if settings.server_url.trim().is_empty() {
        return Err("server URL is required".to_string());
    }
    let client = match settings.operator_api_token.as_deref() {
        Some(token) if !token.trim().is_empty() => {
            OrcasServerClient::with_operator_api_token(settings.server_url.clone(), token)
        }
        _ => OrcasServerClient::new(settings.server_url.clone()),
    };
    Ok(client)
}

fn configured_origin(settings: &OperatorServerSettings) -> Result<&str, String> {
    let origin = settings.origin_node_id.trim();
    if origin.is_empty() {
        return Err("origin node id is required".to_string());
    }
    Ok(origin)
}

fn command_metadata(settings: &OperatorServerSettings) -> Result<authority::CommandMetadata, String> {
    let origin = authority::OriginNodeId::parse(configured_origin(settings)?.to_string())
        .map_err(|error| error.to_string())?;
    let actor =
        authority::CommandActor::parse("operator_web").map_err(|error| error.to_string())?;
    Ok(authority::CommandMetadata::new(origin, actor))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserPushStatusView {
    pub browser_instance_id: String,
    pub recipient_id: String,
    pub subscription_id: String,
    pub service_worker_registered: bool,
    pub notification_permission: BrowserNotificationPermission,
    pub browser_subscription: Option<BrowserPushSubscriptionSnapshot>,
    pub server_subscription_enabled: Option<bool>,
}

fn browser_push_identity(
    settings: &OperatorServerSettings,
) -> Result<storage::BrowserPushIdentity, String> {
    let _ = configured_origin(settings)?;
    let identity = storage::load_browser_push_identity();
    storage::save_browser_push_identity(&identity);
    Ok(identity)
}

fn browser_push_display_name(origin: &str, identity: &storage::BrowserPushIdentity) -> String {
    let short = identity
        .browser_instance_id
        .split('-')
        .next()
        .unwrap_or(&identity.browser_instance_id);
    format!("Orcas web browser {origin} ({short})")
}

fn browser_push_endpoint_payload(
    subscription: &BrowserPushSubscriptionSnapshot,
) -> serde_json::Value {
    serde_json::json!({
        "endpoint": subscription.endpoint,
        "keys": {
            "auth": subscription.auth,
            "p256dh": subscription.p256dh,
        }
    })
}

pub fn inbox_checkpoint_advance(
    after_sequence: u64,
    response: &OperatorInboxWaitForCheckpointResponse,
) -> Option<u64> {
    if response.timed_out {
        return None;
    }
    (response.checkpoint.current_sequence > after_sequence)
        .then_some(response.checkpoint.current_sequence)
}

async fn sync_browser_push_subscription(
    settings: OperatorServerSettings,
    browser_state: BrowserPushState,
) -> Result<BrowserPushStatusView, String> {
    let origin = configured_origin(&settings)?.to_string();
    let client = client_from_settings(&settings)?;
    let identity = browser_push_identity(&settings)?;
    let recipient_id = storage::browser_push_recipient_id(&origin, &identity);
    let subscription_id = storage::browser_push_subscription_id(&origin, &identity);
    let display_name = browser_push_display_name(&origin, &identity);

    let recipient = client
        .notification_recipient_upsert(&NotificationRecipientUpsertRequest {
            recipient_id: recipient_id.clone(),
            display_name,
            enabled: true,
        })
        .await
        .map_err(|error| error.to_string())?
        .recipient;

    let server_subscription_enabled =
        if let Some(subscription) = browser_state.subscription.as_ref() {
            let request = NotificationSubscriptionUpsertRequest {
                subscription_id: subscription_id.clone(),
                recipient_id: recipient.recipient_id.clone(),
                transport_kind: NotificationTransportKind::WebPush,
                endpoint: browser_push_endpoint_payload(subscription),
                enabled: true,
            };
            let subscription = client
                .notification_subscription_upsert(&request)
                .await
                .map_err(|error| error.to_string())?
                .subscription;
            Some(subscription.enabled)
        } else {
            None
        };

    let server_subscription_enabled = client
        .notification_subscription_list(&NotificationSubscriptionListRequest {
            recipient_id: Some(recipient_id.clone()),
            enabled_only: false,
        })
        .await
        .map_err(|error| error.to_string())?
        .subscriptions
        .into_iter()
        .find(|subscription| subscription.subscription_id == subscription_id)
        .map(|subscription| subscription.enabled)
        .or(server_subscription_enabled);

    Ok(BrowserPushStatusView {
        browser_instance_id: identity.browser_instance_id,
        recipient_id,
        subscription_id,
        service_worker_registered: browser_state.service_worker_registered,
        notification_permission: browser_state.notification_permission,
        browser_subscription: browser_state.subscription,
        server_subscription_enabled,
    })
}

pub async fn load_browser_push_status(
    settings: OperatorServerSettings,
) -> Result<BrowserPushStatusView, String> {
    let origin = configured_origin(&settings)?;
    let identity = browser_push_identity(&settings)?;
    let recipient_id = storage::browser_push_recipient_id(origin, &identity);
    let subscription_id = storage::browser_push_subscription_id(origin, &identity);
    let browser_state = pwa::inspect_browser_push_state().await?;
    let client = client_from_settings(&settings)?;
    let server_subscription_enabled = client
        .notification_subscription_list(&NotificationSubscriptionListRequest {
            recipient_id: Some(recipient_id.clone()),
            enabled_only: false,
        })
        .await
        .map_err(|error| error.to_string())?
        .subscriptions
        .into_iter()
        .find(|subscription| subscription.subscription_id == subscription_id)
        .map(|subscription| subscription.enabled);
    Ok(BrowserPushStatusView {
        browser_instance_id: identity.browser_instance_id,
        recipient_id,
        subscription_id,
        service_worker_registered: browser_state.service_worker_registered,
        notification_permission: browser_state.notification_permission,
        browser_subscription: browser_state.subscription,
        server_subscription_enabled,
    })
}

pub async fn register_browser_push_subscription(
    settings: OperatorServerSettings,
) -> Result<BrowserPushStatusView, String> {
    let browser_state =
        pwa::register_browser_push_subscription(settings.push_public_key.clone()).await?;
    sync_browser_push_subscription(settings, browser_state).await
}

pub async fn disable_browser_push_subscription(
    settings: OperatorServerSettings,
) -> Result<BrowserPushStatusView, String> {
    let browser_state = pwa::disable_browser_push_subscription().await?;
    let origin = configured_origin(&settings)?.to_string();
    let client = client_from_settings(&settings)?;
    let identity = browser_push_identity(&settings)?;
    let recipient_id = storage::browser_push_recipient_id(&origin, &identity);
    let subscription_id = storage::browser_push_subscription_id(&origin, &identity);
    if client
        .notification_subscription_list(&NotificationSubscriptionListRequest {
            recipient_id: Some(recipient_id.clone()),
            enabled_only: false,
        })
        .await
        .map_err(|error| error.to_string())?
        .subscriptions
        .into_iter()
        .any(|subscription| subscription.subscription_id == subscription_id)
    {
        let _ = client
            .notification_subscription_set_enabled(&NotificationSubscriptionSetEnabledRequest {
                subscription_id: subscription_id.clone(),
                enabled: false,
            })
            .await
            .map_err(|error| error.to_string())?;
    }
    sync_browser_push_subscription(settings, browser_state).await
}

pub async fn load_inbox_page(settings: OperatorServerSettings) -> Result<InboxPageView, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .operator_inbox_list(origin)
        .await
        .map_err(|error| error.to_string())?;
    Ok(build_inbox_page(response.origin_node_id, &response.items))
}

pub async fn load_workstreams_dashboard(
    settings: OperatorServerSettings,
) -> Result<WorkstreamsDashboardData, String> {
    let client = client_from_settings(&settings)?;
    let hierarchy = client
        .authority_hierarchy_get(&AuthorityHierarchyGetRequest::default())
        .await
        .map_err(|error| error.to_string())?
        .hierarchy;
    let snapshot = client
        .state_get(&StateGetRequest::default())
        .await
        .map_err(|error| error.to_string())?
        .snapshot;
    Ok(WorkstreamsDashboardData { hierarchy, snapshot })
}

pub async fn create_workstream(
    settings: OperatorServerSettings,
    title: String,
    objective: String,
    status: orcas_core::WorkstreamStatus,
    priority: String,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    client
        .authority_workstream_create(&AuthorityWorkstreamCreateRequest {
            command: authority::CreateWorkstream {
                metadata: command_metadata(&settings)?,
                workstream_id: authority::WorkstreamId::new(),
                title,
                objective,
                status,
                priority,
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn edit_workstream(
    settings: OperatorServerSettings,
    workstream_id: authority::WorkstreamId,
    expected_revision: authority::Revision,
    title: String,
    objective: String,
    status: orcas_core::WorkstreamStatus,
    priority: String,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    client
        .authority_workstream_edit(&AuthorityWorkstreamEditRequest {
            command: authority::EditWorkstream {
                metadata: command_metadata(&settings)?,
                workstream_id,
                expected_revision,
                changes: authority::WorkstreamPatch {
                    title: Some(title),
                    objective: Some(objective),
                    status: Some(status),
                    priority: Some(priority),
                },
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn delete_workstream(
    settings: OperatorServerSettings,
    workstream_id: authority::WorkstreamId,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    let delete_plan = client
        .authority_delete_plan(&AuthorityDeletePlanRequest {
            target: authority::DeleteTarget::Workstream {
                workstream_id: workstream_id.clone(),
            },
        })
        .await
        .map_err(|error| error.to_string())?
        .delete_plan;
    client
        .authority_workstream_delete(&orcas_core::ipc::AuthorityWorkstreamDeleteRequest {
            command: authority::DeleteWorkstream {
                metadata: command_metadata(&settings)?,
                workstream_id,
                expected_revision: delete_plan.expected_revision,
                delete_token: delete_plan.confirmation_token,
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn load_work_unit(
    settings: OperatorServerSettings,
    work_unit_id: authority::WorkUnitId,
) -> Result<authority::WorkUnitRecord, String> {
    let client = client_from_settings(&settings)?;
    client
        .authority_workunit_get(&AuthorityWorkunitGetRequest { work_unit_id })
        .await
        .map_err(|error| error.to_string())
        .map(|response| response.work_unit)
}

pub async fn create_work_unit(
    settings: OperatorServerSettings,
    workstream_id: authority::WorkstreamId,
    title: String,
    task_statement: String,
    status: orcas_core::WorkUnitStatus,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    client
        .authority_workunit_create(&AuthorityWorkunitCreateRequest {
            command: authority::CreateWorkUnit {
                metadata: command_metadata(&settings)?,
                work_unit_id: authority::WorkUnitId::new(),
                workstream_id,
                title,
                task_statement,
                status,
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn edit_work_unit(
    settings: OperatorServerSettings,
    work_unit_id: authority::WorkUnitId,
    expected_revision: authority::Revision,
    title: String,
    task_statement: String,
    status: orcas_core::WorkUnitStatus,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    client
        .authority_workunit_edit(&AuthorityWorkunitEditRequest {
            command: authority::EditWorkUnit {
                metadata: command_metadata(&settings)?,
                work_unit_id,
                expected_revision,
                changes: authority::WorkUnitPatch {
                    title: Some(title),
                    task_statement: Some(task_statement),
                    status: Some(status),
                },
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn delete_work_unit(
    settings: OperatorServerSettings,
    work_unit_id: authority::WorkUnitId,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    let delete_plan = client
        .authority_delete_plan(&AuthorityDeletePlanRequest {
            target: authority::DeleteTarget::WorkUnit {
                work_unit_id: work_unit_id.clone(),
            },
        })
        .await
        .map_err(|error| error.to_string())?
        .delete_plan;
    client
        .authority_workunit_delete(&AuthorityWorkunitDeleteRequest {
            command: authority::DeleteWorkUnit {
                metadata: command_metadata(&settings)?,
                work_unit_id,
                expected_revision: delete_plan.expected_revision,
                delete_token: delete_plan.confirmation_token,
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn create_tracked_thread(
    settings: OperatorServerSettings,
    work_unit_id: authority::WorkUnitId,
    title: String,
    upstream_thread_id: Option<String>,
    notes: Option<String>,
    preferred_cwd: Option<String>,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    client
        .authority_tracked_thread_create(&AuthorityTrackedThreadCreateRequest {
            command: authority::CreateTrackedThread {
                metadata: command_metadata(&settings)?,
                tracked_thread_id: authority::TrackedThreadId::new(),
                work_unit_id,
                title,
                notes,
                backend_kind: authority::TrackedThreadBackendKind::Codex,
                upstream_thread_id: upstream_thread_id.filter(|value| !value.trim().is_empty()),
                preferred_cwd: preferred_cwd.filter(|value| !value.trim().is_empty()),
                preferred_model: None,
                workspace: None,
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn bind_tracked_thread(
    settings: OperatorServerSettings,
    tracked_thread_id: authority::TrackedThreadId,
    expected_revision: authority::Revision,
    upstream_thread_id: String,
    preferred_cwd: Option<String>,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    client
        .authority_tracked_thread_edit(&AuthorityTrackedThreadEditRequest {
            command: authority::EditTrackedThread {
                metadata: command_metadata(&settings)?,
                tracked_thread_id,
                expected_revision,
                changes: authority::TrackedThreadPatch {
                    title: None,
                    notes: None,
                    backend_kind: None,
                    upstream_thread_id: Some(Some(upstream_thread_id)),
                    binding_state: Some(authority::TrackedThreadBindingState::Bound),
                    preferred_cwd: Some(preferred_cwd.filter(|value| !value.trim().is_empty())),
                    preferred_model: None,
                    last_seen_turn_id: None,
                    workspace: None,
                },
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn assignment_start(
    settings: OperatorServerSettings,
    work_unit_id: String,
    worker_id: String,
    cwd: Option<String>,
    model: Option<String>,
    instructions: Option<String>,
) -> Result<AssignmentStartResponse, String> {
    let client = client_from_settings(&settings)?;
    client
        .assignment_start(&AssignmentStartRequest {
            work_unit_id,
            worker_id,
            worker_kind: Some("codex".to_string()),
            instructions: instructions.filter(|value| !value.trim().is_empty()),
            model: model.filter(|value| !value.trim().is_empty()),
            cwd: cwd.filter(|value| !value.trim().is_empty()),
            plan_id: None,
            plan_version: None,
            plan_item_id: None,
            execution_kind: orcas_core::planning::PlanExecutionKind::DirectExecution,
            alignment_rationale: None,
        })
        .await
        .map_err(|error| error.to_string())
}

pub async fn proposal_create(
    settings: OperatorServerSettings,
    work_unit_id: String,
    source_report_id: Option<String>,
    note: Option<String>,
) -> Result<ProposalCreateResponse, String> {
    let client = client_from_settings(&settings)?;
    client
        .proposal_create(&ProposalCreateRequest {
            work_unit_id,
            source_report_id,
            requested_by: Some("operator_web".to_string()),
            note: note.filter(|value| !value.trim().is_empty()),
            supersede_open: false,
        })
        .await
        .map_err(|error| error.to_string())
}

pub async fn delete_tracked_thread(
    settings: OperatorServerSettings,
    tracked_thread_id: authority::TrackedThreadId,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    let delete_plan = client
        .authority_delete_plan(&AuthorityDeletePlanRequest {
            target: authority::DeleteTarget::TrackedThread {
                tracked_thread_id: tracked_thread_id.clone(),
            },
        })
        .await
        .map_err(|error| error.to_string())?
        .delete_plan;
    client
        .authority_tracked_thread_delete(&AuthorityTrackedThreadDeleteRequest {
            command: authority::DeleteTrackedThread {
                metadata: command_metadata(&settings)?,
                tracked_thread_id,
                expected_revision: delete_plan.expected_revision,
                delete_token: delete_plan.confirmation_token,
            },
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn load_thread_detail(
    settings: OperatorServerSettings,
    thread_id: String,
) -> Result<ThreadGetResponse, String> {
    let client = client_from_settings(&settings)?;
    client
        .thread_get(&ThreadGetRequest { thread_id })
        .await
        .map_err(|error| error.to_string())
}

pub async fn pause_codex_assignment(
    settings: OperatorServerSettings,
    assignment_id: String,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    client
        .codex_assignment_pause(&CodexAssignmentPauseRequest {
            assignment_id,
            notes: Some("Paused from operator web".to_string()),
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn resume_codex_assignment(
    settings: OperatorServerSettings,
    assignment_id: String,
) -> Result<(), String> {
    let client = client_from_settings(&settings)?;
    client
        .codex_assignment_resume(&CodexAssignmentResumeRequest {
            assignment_id,
            notes: Some("Resumed from operator web".to_string()),
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub async fn load_inbox_item_detail(
    settings: OperatorServerSettings,
    item_id: String,
) -> Result<InboxDetailPageView, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let item = client
        .operator_inbox_get(origin, &item_id)
        .await
        .map_err(|error| error.to_string())?
        .item;
    let notification_candidates = client
        .notification_list(&OperatorNotificationListRequest {
            origin_node_id: origin.to_string(),
            pending_only: false,
            actionable_only: false,
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?
        .candidates;
    let delivery_jobs = client
        .delivery_job_list(&NotificationDeliveryJobListRequest {
            origin_node_id: Some(origin.to_string()),
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?
        .jobs;
    let remote_actions = client
        .remote_action_list(&OperatorRemoteActionListRequest {
            origin_node_id: origin.to_string(),
            item_id: Some(item_id.clone()),
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?
        .requests;
    Ok(build_inbox_detail_page(
        item,
        &notification_candidates,
        &delivery_jobs,
        &remote_actions,
    ))
}

pub async fn load_notifications_page(
    settings: OperatorServerSettings,
) -> Result<NotificationPageView, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .notification_list(&OperatorNotificationListRequest {
            origin_node_id: origin.to_string(),
            pending_only: false,
            actionable_only: false,
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(build_notification_page(
        response.origin_node_id,
        &response.candidates,
    ))
}

pub async fn load_notification_checkpoint(
    settings: OperatorServerSettings,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .notification_checkpoint(&OperatorReadModelCheckpointQueryRequest {
            origin_node_id: origin.to_string(),
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(response.checkpoint.updated_at)
}

pub async fn wait_for_notification_checkpoint(
    settings: OperatorServerSettings,
    after_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    timeout_ms: Option<u64>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .notification_wait_for_checkpoint(&OperatorReadModelWaitForCheckpointRequest {
            origin_node_id: origin.to_string(),
            after_updated_at,
            timeout_ms,
        })
        .await
        .map_err(|error| error.to_string())?;
    if response.timed_out {
        Ok(None)
    } else {
        Ok(response.checkpoint.updated_at)
    }
}

pub async fn load_deliveries_page(
    settings: OperatorServerSettings,
) -> Result<DeliveryPageView, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .delivery_job_list(&NotificationDeliveryJobListRequest {
            origin_node_id: Some(origin.to_string()),
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(build_delivery_page(&response.jobs))
}

pub async fn load_delivery_checkpoint(
    settings: OperatorServerSettings,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .delivery_checkpoint(&OperatorReadModelCheckpointQueryRequest {
            origin_node_id: origin.to_string(),
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(response.checkpoint.updated_at)
}

pub async fn wait_for_delivery_checkpoint(
    settings: OperatorServerSettings,
    after_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    timeout_ms: Option<u64>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .delivery_wait_for_checkpoint(&OperatorReadModelWaitForCheckpointRequest {
            origin_node_id: origin.to_string(),
            after_updated_at,
            timeout_ms,
        })
        .await
        .map_err(|error| error.to_string())?;
    if response.timed_out {
        Ok(None)
    } else {
        Ok(response.checkpoint.updated_at)
    }
}

pub async fn load_action_requests_page(
    settings: OperatorServerSettings,
) -> Result<RemoteActionPageView, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .remote_action_list(&OperatorRemoteActionListRequest {
            origin_node_id: origin.to_string(),
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(build_remote_action_page(&response.requests))
}

pub async fn load_action_request(
    settings: OperatorServerSettings,
    request_id: String,
) -> Result<Option<orcas_operator_core::RemoteActionRequestView>, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .remote_action_get(&OperatorRemoteActionGetRequest {
            origin_node_id: origin.to_string(),
            request_id,
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(response
        .request
        .map(orcas_operator_core::remote_action_request_view))
}

pub async fn submit_remote_action(
    settings: OperatorServerSettings,
    item_id: String,
    action_kind: orcas_core::ipc::OperatorInboxActionKind,
    requested_by: Option<String>,
    request_note: Option<String>,
    idempotency_key: Option<String>,
) -> Result<orcas_operator_core::RemoteActionRequestView, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .remote_action_create(OperatorRemoteActionCreateRequest {
            origin_node_id: origin.to_string(),
            item_id,
            action_kind,
            requested_by,
            request_note,
            idempotency_key,
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(orcas_operator_core::remote_action_request_view(
        response.request,
    ))
}

pub async fn wait_for_remote_action_update(
    settings: OperatorServerSettings,
    request_id: String,
    after_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    timeout_ms: Option<u64>,
) -> Result<Option<orcas_operator_core::RemoteActionRequestView>, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .wait_for_remote_action_update(&OperatorRemoteActionWaitRequest {
            origin_node_id: origin.to_string(),
            request_id,
            after_updated_at,
            timeout_ms,
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(response
        .request
        .map(orcas_operator_core::remote_action_request_view))
}

pub async fn wait_for_remote_action_checkpoint(
    settings: OperatorServerSettings,
    request_id: String,
    after_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    timeout_ms: Option<u64>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let response =
        wait_for_remote_action_update(settings, request_id, after_updated_at, timeout_ms).await?;
    Ok(response.map(|request| request.updated_at))
}

pub async fn wait_for_inbox_checkpoint(
    settings: OperatorServerSettings,
    after_sequence: Option<u64>,
    timeout_ms: Option<u64>,
) -> Result<orcas_core::ipc::OperatorInboxWaitForCheckpointResponse, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    client
        .inbox_wait_for_checkpoint(&OperatorInboxWaitForCheckpointRequest {
            origin_node_id: origin.to_string(),
            after_sequence,
            timeout_ms,
        })
        .await
        .map_err(|error| error.to_string())
}

pub async fn inbox_checkpoint(
    settings: OperatorServerSettings,
) -> Result<orcas_core::ipc::OperatorInboxMirrorCheckpointQueryResponse, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    client
        .inbox_checkpoint(origin)
        .await
        .map_err(|error| error.to_string())
}

pub fn generated_idempotency_key() -> String {
    Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn generated_idempotency_key_is_nonempty_and_uuid_like() {
        let key = generated_idempotency_key();
        assert!(!key.trim().is_empty());
        assert!(Uuid::parse_str(&key).is_ok());
    }

    #[test]
    fn browser_push_identity_is_stable_and_scoped() {
        let identity = storage::BrowserPushIdentity {
            browser_instance_id: "browser-123".to_string(),
        };
        assert_eq!(
            storage::browser_push_recipient_id("origin-a", &identity),
            "browser::origin-a::browser-123"
        );
        assert_eq!(
            storage::browser_push_subscription_id("origin-a", &identity),
            "browser::origin-a::browser-123::webpush"
        );
    }

    #[test]
    fn browser_push_endpoint_payload_includes_keys() {
        let payload = browser_push_endpoint_payload(&BrowserPushSubscriptionSnapshot {
            endpoint: "https://example.invalid/push".to_string(),
            auth: Some("auth".to_string()),
            p256dh: Some("p256dh".to_string()),
        });
        assert_eq!(payload["endpoint"], "https://example.invalid/push");
        assert_eq!(payload["keys"]["auth"], "auth");
        assert_eq!(payload["keys"]["p256dh"], "p256dh");
    }

    #[test]
    fn remote_action_idempotency_key_is_stable_for_the_same_item_state() {
        let updated_at = chrono::Utc
            .with_ymd_and_hms(2026, 3, 22, 1, 42, 17)
            .single()
            .expect("timestamp");
        let first = storage::remote_action_idempotency_key(
            "origin-a",
            "planning_session::session-1",
            orcas_core::ipc::OperatorInboxActionKind::Approve,
            updated_at,
        );
        let second = storage::remote_action_idempotency_key(
            "origin-a",
            "planning_session::session-1",
            orcas_core::ipc::OperatorInboxActionKind::Approve,
            updated_at,
        );
        let different = storage::remote_action_idempotency_key(
            "origin-a",
            "planning_session::session-1",
            orcas_core::ipc::OperatorInboxActionKind::Reject,
            updated_at,
        );

        assert_eq!(first, second);
        assert_ne!(first, different);
    }

    #[test]
    fn inbox_checkpoint_wait_only_advances_on_new_sequence() {
        let current = orcas_core::ipc::OperatorInboxWaitForCheckpointResponse {
            origin_node_id: "origin-a".to_string(),
            checkpoint: orcas_core::ipc::OperatorInboxCheckpoint {
                current_sequence: 7,
                updated_at: chrono::Utc::now(),
            },
            timed_out: false,
        };
        assert_eq!(inbox_checkpoint_advance(6, &current), Some(7));
        assert_eq!(inbox_checkpoint_advance(7, &current), None);
        let timed_out = orcas_core::ipc::OperatorInboxWaitForCheckpointResponse {
            timed_out: true,
            ..current
        };
        assert_eq!(inbox_checkpoint_advance(6, &timed_out), None);
    }
}
