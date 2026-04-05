use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use tt_core::ipc::{
    NotificationDeliveryJobGetRequest, NotificationDeliveryJobListRequest, OperatorInboxActionKind,
    OperatorInboxItemStatus, OperatorInboxSourceKind, OperatorNotificationAckRequest,
    OperatorNotificationCandidateStatus, OperatorNotificationGetRequest,
    OperatorNotificationListRequest, OperatorNotificationSuppressRequest,
    OperatorRemoteActionCreateRequest, OperatorRemoteActionGetRequest,
    OperatorRemoteActionListRequest, OperatorRemoteActionRequestStatus,
    OperatorRemoteActionWaitRequest,
};
use tt_server_client::TTServerClient;
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    Inbox {
        #[command(subcommand)]
        command: RemoteInboxCommand,
    },
    Notifications {
        #[command(subcommand)]
        command: RemoteNotificationCommand,
    },
    Deliveries {
        #[command(subcommand)]
        command: RemoteDeliveryCommand,
    },
    Actions {
        #[command(subcommand)]
        command: RemoteActionCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum RemoteInboxCommand {
    List(RemoteInboxListArgs),
    Get(RemoteInboxGetArgs),
}

#[derive(Debug, Subcommand)]
pub enum RemoteNotificationCommand {
    List(RemoteNotificationListArgs),
    Get(RemoteNotificationGetArgs),
    Ack(RemoteNotificationAckArgs),
    Suppress(RemoteNotificationSuppressArgs),
}

#[derive(Debug, Subcommand)]
pub enum RemoteDeliveryCommand {
    List(RemoteDeliveryListArgs),
    Get(RemoteDeliveryGetArgs),
}

#[derive(Debug, Subcommand)]
pub enum RemoteActionCommand {
    Submit(RemoteActionSubmitArgs),
    List(RemoteActionListArgs),
    Get(RemoteActionGetArgs),
    Watch(RemoteActionWatchArgs),
}

#[derive(Debug, Clone, Args)]
pub struct RemoteInboxListArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long, value_enum)]
    pub source_kind: Option<RemoteInboxSourceKindArg>,
    #[arg(long, default_value_t = true)]
    pub actionable_only: bool,
    #[arg(long, default_value_t = false)]
    pub include_closed: bool,
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteInboxGetArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long = "item")]
    pub item_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteNotificationListArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long)]
    pub status: Option<RemoteNotificationStatusArg>,
    #[arg(long, default_value_t = true)]
    pub pending_only: bool,
    #[arg(long, default_value_t = true)]
    pub actionable_only: bool,
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteNotificationGetArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long = "candidate")]
    pub candidate_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteNotificationAckArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long = "candidate")]
    pub candidate_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteNotificationSuppressArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long = "candidate")]
    pub candidate_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteDeliveryListArgs {
    #[arg(long = "origin")]
    pub origin_node_id: Option<String>,
    #[arg(long = "candidate")]
    pub candidate_id: Option<String>,
    #[arg(long = "subscription")]
    pub subscription_id: Option<String>,
    #[arg(long)]
    pub status: Option<RemoteDeliveryStatusArg>,
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteDeliveryGetArgs {
    #[arg(long = "job")]
    pub job_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteActionSubmitArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long = "item")]
    pub item_id: String,
    #[arg(long = "action", value_enum)]
    pub action_kind: RemoteActionKindArg,
    #[arg(long = "requested-by")]
    pub requested_by: Option<String>,
    #[arg(long = "note")]
    pub request_note: Option<String>,
    #[arg(long = "idempotency-key")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteActionListArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long = "candidate")]
    pub candidate_id: Option<String>,
    #[arg(long = "item")]
    pub item_id: Option<String>,
    #[arg(long = "action", value_enum)]
    pub action_kind: Option<RemoteActionKindArg>,
    #[arg(long)]
    pub status: Option<RemoteActionStatusArg>,
    #[arg(long, default_value_t = false)]
    pub pending_only: bool,
    #[arg(long, default_value_t = false)]
    pub actionable_only: bool,
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteActionGetArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long = "request")]
    pub request_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct RemoteActionWatchArgs {
    #[arg(long = "origin")]
    pub origin_node_id: String,
    #[arg(long = "request")]
    pub request_id: String,
    #[arg(long)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum RemoteInboxSourceKindArg {
    SupervisorProposal,
    SupervisorDecision,
    PlanningSession,
    PlanRevisionProposal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum RemoteNotificationStatusArg {
    Pending,
    Acknowledged,
    Suppressed,
    Obsolete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum RemoteDeliveryStatusArg {
    Pending,
    Dispatched,
    Delivered,
    Failed,
    Suppressed,
    Skipped,
    Obsolete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum RemoteActionKindArg {
    Approve,
    Reject,
    ApproveAndSend,
    RecordNoAction,
    ManualRefresh,
    Reconcile,
    Retry,
    Supersede,
    MarkReadyForReview,
}

fn server_client(global: &super::GlobalOptions) -> Result<TTServerClient> {
    let base_url = global
        .server_url
        .as_deref()
        .context("`--server-url` or `TT_SERVER_URL` is required for remote commands")?;
    Ok(match global.operator_api_token.as_deref() {
        Some(token) => TTServerClient::with_operator_api_token(base_url, token),
        None => TTServerClient::new(base_url),
    })
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn remote_action_kind(kind: RemoteActionKindArg) -> OperatorInboxActionKind {
    match kind {
        RemoteActionKindArg::Approve => OperatorInboxActionKind::Approve,
        RemoteActionKindArg::Reject => OperatorInboxActionKind::Reject,
        RemoteActionKindArg::ApproveAndSend => OperatorInboxActionKind::ApproveAndSend,
        RemoteActionKindArg::RecordNoAction => OperatorInboxActionKind::RecordNoAction,
        RemoteActionKindArg::ManualRefresh => OperatorInboxActionKind::ManualRefresh,
        RemoteActionKindArg::Reconcile => OperatorInboxActionKind::Reconcile,
        RemoteActionKindArg::Retry => OperatorInboxActionKind::Retry,
        RemoteActionKindArg::Supersede => OperatorInboxActionKind::Supersede,
        RemoteActionKindArg::MarkReadyForReview => OperatorInboxActionKind::MarkReadyForReview,
    }
}

fn inbox_source_kind(kind: RemoteInboxSourceKindArg) -> OperatorInboxSourceKind {
    match kind {
        RemoteInboxSourceKindArg::SupervisorProposal => OperatorInboxSourceKind::SupervisorProposal,
        RemoteInboxSourceKindArg::SupervisorDecision => OperatorInboxSourceKind::SupervisorDecision,
        RemoteInboxSourceKindArg::PlanningSession => OperatorInboxSourceKind::PlanningSession,
        RemoteInboxSourceKindArg::PlanRevisionProposal => {
            OperatorInboxSourceKind::PlanRevisionProposal
        }
    }
}

fn notification_status(kind: RemoteNotificationStatusArg) -> OperatorNotificationCandidateStatus {
    match kind {
        RemoteNotificationStatusArg::Pending => OperatorNotificationCandidateStatus::Pending,
        RemoteNotificationStatusArg::Acknowledged => {
            OperatorNotificationCandidateStatus::Acknowledged
        }
        RemoteNotificationStatusArg::Suppressed => OperatorNotificationCandidateStatus::Suppressed,
        RemoteNotificationStatusArg::Obsolete => OperatorNotificationCandidateStatus::Obsolete,
    }
}

fn delivery_status(kind: RemoteDeliveryStatusArg) -> tt_core::ipc::NotificationDeliveryJobStatus {
    match kind {
        RemoteDeliveryStatusArg::Pending => tt_core::ipc::NotificationDeliveryJobStatus::Pending,
        RemoteDeliveryStatusArg::Dispatched => {
            tt_core::ipc::NotificationDeliveryJobStatus::Dispatched
        }
        RemoteDeliveryStatusArg::Delivered => {
            tt_core::ipc::NotificationDeliveryJobStatus::Delivered
        }
        RemoteDeliveryStatusArg::Failed => tt_core::ipc::NotificationDeliveryJobStatus::Failed,
        RemoteDeliveryStatusArg::Suppressed => {
            tt_core::ipc::NotificationDeliveryJobStatus::Suppressed
        }
        RemoteDeliveryStatusArg::Skipped => tt_core::ipc::NotificationDeliveryJobStatus::Skipped,
        RemoteDeliveryStatusArg::Obsolete => tt_core::ipc::NotificationDeliveryJobStatus::Obsolete,
    }
}

async fn watch_remote_action(
    client: &TTServerClient,
    origin_node_id: &str,
    request_id: &str,
    timeout_ms: Option<u64>,
) -> Result<()> {
    let mut current = client
        .remote_action_get(&OperatorRemoteActionGetRequest {
            origin_node_id: origin_node_id.to_string(),
            request_id: request_id.to_string(),
        })
        .await?
        .request
        .with_context(|| {
            format!("remote action `{request_id}` not found for origin `{origin_node_id}`")
        })?;
    print_json(&current)?;
    while !matches!(
        current.status,
        OperatorRemoteActionRequestStatus::Completed
            | OperatorRemoteActionRequestStatus::Failed
            | OperatorRemoteActionRequestStatus::Canceled
            | OperatorRemoteActionRequestStatus::Stale
    ) {
        let response = client
            .remote_action_wait(&OperatorRemoteActionWaitRequest {
                origin_node_id: origin_node_id.to_string(),
                request_id: request_id.to_string(),
                after_updated_at: Some(current.updated_at),
                timeout_ms,
            })
            .await?;
        if response.timed_out {
            continue;
        }
        if let Some(next) = response.request {
            if next.updated_at > current.updated_at {
                current = next;
                print_json(&current)?;
            }
        }
    }
    Ok(())
}

pub async fn run_remote(global: &super::GlobalOptions, command: RemoteCommand) -> Result<()> {
    let client = server_client(global)?;
    match command {
        RemoteCommand::Inbox { command } => match command {
            RemoteInboxCommand::List(args) => {
                let mut response = client.operator_inbox_list(&args.origin_node_id).await?;
                if let Some(kind) = args.source_kind {
                    let kind = inbox_source_kind(kind);
                    response.items.retain(|item| item.source_kind == kind);
                }
                if args.actionable_only {
                    response
                        .items
                        .retain(|item| !item.available_actions.is_empty());
                }
                if !args.include_closed {
                    response
                        .items
                        .retain(|item| item.status == OperatorInboxItemStatus::Open);
                }
                if let Some(limit) = args.limit {
                    response.items.truncate(limit);
                }
                print_json(&response)?;
            }
            RemoteInboxCommand::Get(args) => {
                let response = client
                    .operator_inbox_get(&args.origin_node_id, &args.item_id)
                    .await?;
                print_json(&response)?;
            }
        },
        RemoteCommand::Notifications { command } => match command {
            RemoteNotificationCommand::List(args) => {
                let response = client
                    .notification_list(&OperatorNotificationListRequest {
                        origin_node_id: args.origin_node_id,
                        status: args.status.map(notification_status),
                        pending_only: args.pending_only,
                        actionable_only: args.actionable_only,
                        limit: args.limit,
                    })
                    .await?;
                print_json(&response)?;
            }
            RemoteNotificationCommand::Get(args) => {
                let response = client
                    .notification_get(&OperatorNotificationGetRequest {
                        origin_node_id: args.origin_node_id,
                        candidate_id: args.candidate_id,
                    })
                    .await?;
                print_json(&response)?;
            }
            RemoteNotificationCommand::Ack(args) => {
                let response = client
                    .notification_ack(&OperatorNotificationAckRequest {
                        origin_node_id: args.origin_node_id,
                        candidate_id: args.candidate_id,
                    })
                    .await?;
                print_json(&response)?;
            }
            RemoteNotificationCommand::Suppress(args) => {
                let response = client
                    .notification_suppress(&OperatorNotificationSuppressRequest {
                        origin_node_id: args.origin_node_id,
                        candidate_id: args.candidate_id,
                    })
                    .await?;
                print_json(&response)?;
            }
        },
        RemoteCommand::Deliveries { command } => match command {
            RemoteDeliveryCommand::List(args) => {
                let response = client
                    .delivery_job_list(&NotificationDeliveryJobListRequest {
                        origin_node_id: args.origin_node_id,
                        candidate_id: args.candidate_id,
                        subscription_id: args.subscription_id,
                        status: args.status.map(delivery_status),
                        limit: args.limit,
                    })
                    .await?;
                print_json(&response)?;
            }
            RemoteDeliveryCommand::Get(args) => {
                let response = client
                    .delivery_job_get(&NotificationDeliveryJobGetRequest {
                        job_id: args.job_id,
                    })
                    .await?;
                print_json(&response)?;
            }
        },
        RemoteCommand::Actions { command } => match command {
            RemoteActionCommand::Submit(args) => {
                let idempotency_key = args
                    .idempotency_key
                    .unwrap_or_else(|| format!("tt-remote-{}", Uuid::now_v7()));
                let response = client
                    .remote_action_create(OperatorRemoteActionCreateRequest {
                        origin_node_id: args.origin_node_id,
                        item_id: args.item_id,
                        action_kind: remote_action_kind(args.action_kind),
                        idempotency_key: Some(idempotency_key),
                        requested_by: args.requested_by,
                        request_note: args.request_note,
                    })
                    .await?;
                print_json(&response)?;
            }
            RemoteActionCommand::List(args) => {
                let response = client
                    .remote_action_list(&OperatorRemoteActionListRequest {
                        origin_node_id: args.origin_node_id,
                        candidate_id: args.candidate_id,
                        item_id: args.item_id,
                        action_kind: args.action_kind.map(remote_action_kind),
                        status: args.status.map(|status| match status {
                            RemoteActionStatusArg::Pending => {
                                OperatorRemoteActionRequestStatus::Pending
                            }
                            RemoteActionStatusArg::Claimed => {
                                OperatorRemoteActionRequestStatus::Claimed
                            }
                            RemoteActionStatusArg::Completed => {
                                OperatorRemoteActionRequestStatus::Completed
                            }
                            RemoteActionStatusArg::Failed => {
                                OperatorRemoteActionRequestStatus::Failed
                            }
                            RemoteActionStatusArg::Canceled => {
                                OperatorRemoteActionRequestStatus::Canceled
                            }
                            RemoteActionStatusArg::Stale => {
                                OperatorRemoteActionRequestStatus::Stale
                            }
                        }),
                        pending_only: args.pending_only,
                        actionable_only: args.actionable_only,
                        limit: args.limit,
                    })
                    .await?;
                print_json(&response)?;
            }
            RemoteActionCommand::Get(args) => {
                let response = client
                    .remote_action_get(&OperatorRemoteActionGetRequest {
                        origin_node_id: args.origin_node_id,
                        request_id: args.request_id,
                    })
                    .await?;
                print_json(&response)?;
            }
            RemoteActionCommand::Watch(args) => {
                watch_remote_action(
                    &client,
                    &args.origin_node_id,
                    &args.request_id,
                    args.timeout_ms,
                )
                .await?;
            }
        },
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum RemoteActionStatusArg {
    Pending,
    Claimed,
    Completed,
    Failed,
    Canceled,
    Stale,
}
