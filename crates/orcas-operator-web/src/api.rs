use orcas_core::ipc::{
    NotificationDeliveryJobListRequest, OperatorInboxWaitForCheckpointRequest,
    OperatorNotificationListRequest, OperatorRemoteActionCreateRequest,
    OperatorRemoteActionGetRequest, OperatorRemoteActionListRequest,
    OperatorRemoteActionWaitRequest,
};
use orcas_operator_core::{
    build_delivery_page, build_inbox_detail_page, build_inbox_page, build_notification_page,
    build_remote_action_page, DeliveryPageView, InboxDetailPageView, InboxPageView,
    NotificationPageView, OperatorServerSettings, RemoteActionPageView,
};
use orcas_server_client::OrcasServerClient;
use uuid::Uuid;

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

pub async fn load_inbox_page(settings: OperatorServerSettings) -> Result<InboxPageView, String> {
    let origin = configured_origin(&settings)?;
    let client = client_from_settings(&settings)?;
    let response = client
        .operator_inbox_list(origin)
        .await
        .map_err(|error| error.to_string())?;
    Ok(build_inbox_page(response.origin_node_id, &response.items))
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
    Ok(build_notification_page(response.origin_node_id, &response.candidates))
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
    Ok(response.request.map(orcas_operator_core::remote_action_request_view))
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
    Ok(orcas_operator_core::remote_action_request_view(response.request))
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
    Ok(response.request.map(orcas_operator_core::remote_action_request_view))
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
    use super::*;

    #[test]
    fn generated_idempotency_key_is_nonempty_and_uuid_like() {
        let key = generated_idempotency_key();
        assert!(!key.trim().is_empty());
        assert!(Uuid::parse_str(&key).is_ok());
    }
}
