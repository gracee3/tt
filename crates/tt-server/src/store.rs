use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use tokio::sync::broadcast;
use tt_core::ipc::{
    NotificationDeliveryJob, NotificationDeliveryJobGetRequest, NotificationDeliveryJobListRequest,
    NotificationDeliveryJobListResponse, NotificationDeliveryJobStatus,
    NotificationDeliveryRunPendingResponse, NotificationRecipient,
    NotificationRecipientListRequest, NotificationRecipientListResponse,
    NotificationRecipientUpsertRequest, NotificationRecipientUpsertResponse,
    NotificationSubscription, NotificationSubscriptionListRequest,
    NotificationSubscriptionListResponse, NotificationSubscriptionSetEnabledRequest,
    NotificationSubscriptionSetEnabledResponse, NotificationSubscriptionUpsertRequest,
    NotificationSubscriptionUpsertResponse, NotificationTransportKind, OperatorInboxChange,
    OperatorInboxCheckpoint, OperatorInboxItem, OperatorInboxMirrorCheckpoint,
    OperatorInboxMirrorListResponse, OperatorNotificationAckRequest,
    OperatorNotificationAckResponse, OperatorNotificationCandidate,
    OperatorNotificationCandidateStatus, OperatorNotificationGetRequest,
    OperatorNotificationListRequest, OperatorNotificationListResponse,
    OperatorNotificationSuppressRequest, OperatorNotificationSuppressResponse,
    OperatorReadModelCheckpoint, OperatorReadModelCheckpointQueryRequest,
    OperatorReadModelCheckpointQueryResponse, OperatorReadModelWaitForCheckpointRequest,
    OperatorReadModelWaitForCheckpointResponse, OperatorRemoteActionClaimRequest,
    OperatorRemoteActionClaimResponse, OperatorRemoteActionClaimedRequest,
    OperatorRemoteActionCompleteRequest, OperatorRemoteActionCompleteResponse,
    OperatorRemoteActionCreateRequest, OperatorRemoteActionCreateResponse,
    OperatorRemoteActionFailRequest, OperatorRemoteActionFailResponse,
    OperatorRemoteActionGetRequest, OperatorRemoteActionGetResponse,
    OperatorRemoteActionListRequest, OperatorRemoteActionListResponse, OperatorRemoteActionRequest,
    OperatorRemoteActionRequestStatus,
};
use tt_core::{TTError, TTResult};
use uuid::Uuid;

use crate::delivery::{NotificationDeliveryContext, NotificationDeliveryTransport};

const INITIAL_SCHEMA: &str = r#"
create table if not exists mirrored_inbox_items (
  origin_node_id text not null,
  item_id text not null,
  sequence integer not null,
  item_json text not null,
  changed_at text not null,
  primary key (origin_node_id, item_id)
);

create table if not exists mirrored_inbox_checkpoint (
  origin_node_id text primary key,
  current_sequence integer not null,
  updated_at text not null
);

create table if not exists mirrored_notification_candidates (
  candidate_id text primary key,
  origin_node_id text not null,
  item_id text not null,
  trigger_sequence integer not null,
  candidate_status text not null,
  item_json text not null,
  created_at text not null,
  updated_at text not null,
  acknowledged_at text,
  suppressed_at text,
  resolved_at text,
  obsolete_at text
);

create index if not exists mirrored_notification_candidates_origin_idx
  on mirrored_notification_candidates(origin_node_id, candidate_status, updated_at desc);

create table if not exists mirrored_notification_windows (
  origin_node_id text not null,
  item_id text not null,
  candidate_id text not null,
  opened_sequence integer not null,
  updated_sequence integer not null,
  updated_at text not null,
  primary key (origin_node_id, item_id)
);

create table if not exists notification_recipients (
  recipient_id text primary key,
  display_name text not null,
  enabled integer not null,
  created_at text not null,
  updated_at text not null
);

create table if not exists notification_subscriptions (
  subscription_id text primary key,
  recipient_id text not null,
  transport_kind text not null,
  endpoint_json text not null,
  enabled integer not null,
  created_at text not null,
  updated_at text not null
);

create index if not exists notification_subscriptions_recipient_idx
  on notification_subscriptions(recipient_id, enabled);

create table if not exists notification_delivery_jobs (
  job_id text primary key,
  origin_node_id text not null,
  candidate_id text not null,
  trigger_sequence integer not null,
  recipient_id text not null,
  subscription_id text not null,
  transport_kind text not null,
  status text not null,
  attempt_count integer not null,
  created_at text not null,
  updated_at text not null,
  dispatched_at text,
  delivered_at text,
  failed_at text,
  suppressed_at text,
  skipped_at text,
  obsolete_at text,
  receipt_json text,
  error_text text
);

create unique index if not exists notification_delivery_jobs_dedupe_idx
  on notification_delivery_jobs(candidate_id, subscription_id, trigger_sequence);

create index if not exists notification_delivery_jobs_status_idx
  on notification_delivery_jobs(status, updated_at desc);

create table if not exists remote_action_requests (
  request_id text primary key,
  origin_node_id text not null,
  candidate_id text not null,
  item_id text not null,
  trigger_sequence integer not null,
  action_kind text not null,
  idempotency_key text,
  item_json text not null,
  requested_by text,
  request_note text,
  request_status text not null,
  created_at text not null,
  updated_at text not null,
  claimed_by text,
  claimed_at text,
  claimed_until text,
  claim_token text,
  completed_at text,
  failed_at text,
  canceled_at text,
  stale_at text,
  attempt_count integer not null,
  result_json text,
  error_text text
);

create index if not exists remote_action_requests_origin_idx
  on remote_action_requests(origin_node_id, request_status, updated_at desc);

create index if not exists remote_action_requests_candidate_idx
  on remote_action_requests(candidate_id, action_kind);
"#;

fn db_error(error: rusqlite::Error) -> TTError {
    TTError::Store(error.to_string())
}

#[derive(Debug)]
pub struct InboxMirrorStore {
    connection: Mutex<Connection>,
    checkpoint_events: broadcast::Sender<()>,
}

#[derive(Debug, Clone)]
pub struct MirrorApplyResult {
    pub checkpoint: OperatorInboxCheckpoint,
    pub mirror_checkpoint: OperatorInboxMirrorCheckpoint,
    pub applied_changes: usize,
    pub skipped_changes: usize,
}

impl InboxMirrorStore {
    pub fn open(path: impl AsRef<Path>) -> TTResult<Self> {
        let connection = Connection::open(path).map_err(db_error)?;
        connection.execute_batch(INITIAL_SCHEMA).map_err(db_error)?;
        let (checkpoint_events, _) = broadcast::channel(32);
        Ok(Self {
            connection: Mutex::new(connection),
            checkpoint_events,
        })
    }

    pub fn subscribe_checkpoint_events(&self) -> broadcast::Receiver<()> {
        self.checkpoint_events.subscribe()
    }

    fn notify_checkpoint_changed(&self) {
        let _ = self.checkpoint_events.send(());
    }

    pub fn checkpoint(&self, origin_node_id: &str) -> TTResult<OperatorInboxCheckpoint> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection.prepare(
            "select current_sequence, updated_at from mirrored_inbox_checkpoint where origin_node_id = ?1",
        ).map_err(db_error)?;
        let checkpoint = statement
            .query_row(params![origin_node_id], |row| {
                Ok(OperatorInboxCheckpoint {
                    current_sequence: row.get::<_, i64>(0)? as u64,
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(1)?)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                })
            })
            .optional()
            .map_err(db_error)?;
        Ok(checkpoint.unwrap_or_default())
    }

    fn load_recipient_tx(
        transaction: &rusqlite::Transaction<'_>,
        recipient_id: &str,
    ) -> TTResult<Option<NotificationRecipient>> {
        let mut statement = transaction
            .prepare(
                "select recipient_id, display_name, enabled, created_at, updated_at
                 from notification_recipients where recipient_id = ?1",
            )
            .map_err(db_error)?;
        let recipient = statement
            .query_row(params![recipient_id], |row| Self::recipient_from_row(row))
            .optional()
            .map_err(db_error)?;
        Ok(recipient)
    }

    fn recipient_from_row(
        row: &rusqlite::Row<'_>,
    ) -> Result<NotificationRecipient, rusqlite::Error> {
        Ok(NotificationRecipient {
            recipient_id: row.get::<_, String>(0)?,
            display_name: row.get::<_, String>(1)?,
            enabled: row.get::<_, i64>(2)? != 0,
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
        })
    }

    pub async fn wait_for_checkpoint(
        &self,
        request: &tt_core::ipc::OperatorInboxWaitForCheckpointRequest,
    ) -> TTResult<tt_core::ipc::OperatorInboxWaitForCheckpointResponse> {
        let timeout = tokio::time::Duration::from_millis(request.timeout_ms.unwrap_or(30_000));
        let after_sequence = request.after_sequence.unwrap_or_default();
        let mut checkpoint = self.checkpoint(request.origin_node_id.as_str())?;
        if checkpoint.current_sequence > after_sequence {
            return Ok(tt_core::ipc::OperatorInboxWaitForCheckpointResponse {
                origin_node_id: request.origin_node_id.clone(),
                checkpoint,
                timed_out: false,
            });
        }

        let mut events = self.subscribe_checkpoint_events();
        let start = tokio::time::Instant::now();
        loop {
            let remaining = timeout.checked_sub(start.elapsed()).unwrap_or_default();
            if remaining.is_zero() {
                checkpoint = self.checkpoint(request.origin_node_id.as_str())?;
                return Ok(tt_core::ipc::OperatorInboxWaitForCheckpointResponse {
                    origin_node_id: request.origin_node_id.clone(),
                    checkpoint,
                    timed_out: true,
                });
            }

            match tokio::time::timeout(remaining, events.recv()).await {
                Ok(Ok(())) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    checkpoint = self.checkpoint(request.origin_node_id.as_str())?;
                    if checkpoint.current_sequence > after_sequence {
                        return Ok(tt_core::ipc::OperatorInboxWaitForCheckpointResponse {
                            origin_node_id: request.origin_node_id.clone(),
                            checkpoint,
                            timed_out: false,
                        });
                    }
                }
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => {
                    checkpoint = self.checkpoint(request.origin_node_id.as_str())?;
                    return Ok(tt_core::ipc::OperatorInboxWaitForCheckpointResponse {
                        origin_node_id: request.origin_node_id.clone(),
                        checkpoint: checkpoint.clone(),
                        timed_out: checkpoint.current_sequence <= after_sequence,
                    });
                }
            }
        }
    }

    fn read_model_checkpoint_from_table(
        &self,
        origin_node_id: &str,
        table_name: &str,
    ) -> TTResult<OperatorReadModelCheckpoint> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection
            .prepare(&format!(
                "select max(updated_at) from {table_name} where origin_node_id = ?1"
            ))
            .map_err(db_error)?;
        let updated_at = statement
            .query_row(params![origin_node_id], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()
            .map_err(db_error)?
            .flatten()
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
            })
            .transpose()
            .map_err(db_error)?;
        Ok(OperatorReadModelCheckpoint { updated_at })
    }

    async fn wait_for_read_model_checkpoint(
        &self,
        origin_node_id: &str,
        after_updated_at: Option<DateTime<Utc>>,
        timeout_ms: Option<u64>,
        table_name: &'static str,
    ) -> TTResult<OperatorReadModelWaitForCheckpointResponse> {
        let timeout = tokio::time::Duration::from_millis(timeout_ms.unwrap_or(30_000).max(1));
        let mut checkpoint = self.read_model_checkpoint_from_table(origin_node_id, table_name)?;
        if checkpoint
            .updated_at
            .is_some_and(|updated_at| after_updated_at.is_none_or(|after| updated_at > after))
        {
            return Ok(OperatorReadModelWaitForCheckpointResponse {
                origin_node_id: origin_node_id.to_string(),
                checkpoint,
                timed_out: false,
            });
        }

        let mut events = self.subscribe_checkpoint_events();
        let start = tokio::time::Instant::now();
        loop {
            let remaining = timeout.checked_sub(start.elapsed()).unwrap_or_default();
            if remaining.is_zero() {
                checkpoint = self.read_model_checkpoint_from_table(origin_node_id, table_name)?;
                return Ok(OperatorReadModelWaitForCheckpointResponse {
                    origin_node_id: origin_node_id.to_string(),
                    checkpoint,
                    timed_out: true,
                });
            }

            match tokio::time::timeout(remaining, events.recv()).await {
                Ok(Ok(())) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    checkpoint =
                        self.read_model_checkpoint_from_table(origin_node_id, table_name)?;
                    if checkpoint.updated_at.is_some_and(|updated_at| {
                        after_updated_at.is_none_or(|after| updated_at > after)
                    }) {
                        return Ok(OperatorReadModelWaitForCheckpointResponse {
                            origin_node_id: origin_node_id.to_string(),
                            checkpoint,
                            timed_out: false,
                        });
                    }
                }
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => {
                    checkpoint =
                        self.read_model_checkpoint_from_table(origin_node_id, table_name)?;
                    return Ok(OperatorReadModelWaitForCheckpointResponse {
                        origin_node_id: origin_node_id.to_string(),
                        checkpoint: checkpoint.clone(),
                        timed_out: checkpoint.updated_at.is_none_or(|updated_at| {
                            after_updated_at.is_none_or(|after| updated_at <= after)
                        }),
                    });
                }
            }
        }
    }

    pub fn notification_checkpoint(
        &self,
        request: &OperatorReadModelCheckpointQueryRequest,
    ) -> TTResult<OperatorReadModelCheckpointQueryResponse> {
        let checkpoint = self.read_model_checkpoint_from_table(
            request.origin_node_id.as_str(),
            "mirrored_notification_candidates",
        )?;
        Ok(OperatorReadModelCheckpointQueryResponse {
            origin_node_id: request.origin_node_id.clone(),
            checkpoint,
        })
    }

    pub async fn wait_for_notification_checkpoint(
        &self,
        request: &OperatorReadModelWaitForCheckpointRequest,
    ) -> TTResult<OperatorReadModelWaitForCheckpointResponse> {
        self.wait_for_read_model_checkpoint(
            request.origin_node_id.as_str(),
            request.after_updated_at,
            request.timeout_ms,
            "mirrored_notification_candidates",
        )
        .await
    }

    pub fn delivery_checkpoint(
        &self,
        request: &OperatorReadModelCheckpointQueryRequest,
    ) -> TTResult<OperatorReadModelCheckpointQueryResponse> {
        let checkpoint = self.read_model_checkpoint_from_table(
            request.origin_node_id.as_str(),
            "notification_delivery_jobs",
        )?;
        Ok(OperatorReadModelCheckpointQueryResponse {
            origin_node_id: request.origin_node_id.clone(),
            checkpoint,
        })
    }

    pub async fn wait_for_delivery_checkpoint(
        &self,
        request: &OperatorReadModelWaitForCheckpointRequest,
    ) -> TTResult<OperatorReadModelWaitForCheckpointResponse> {
        self.wait_for_read_model_checkpoint(
            request.origin_node_id.as_str(),
            request.after_updated_at,
            request.timeout_ms,
            "notification_delivery_jobs",
        )
        .await
    }

    fn load_subscription_tx(
        transaction: &rusqlite::Transaction<'_>,
        subscription_id: &str,
    ) -> TTResult<Option<NotificationSubscription>> {
        let mut statement = transaction
            .prepare(
                "select subscription_id, recipient_id, transport_kind, endpoint_json, enabled, created_at, updated_at
                 from notification_subscriptions where subscription_id = ?1",
            )
            .map_err(db_error)?;
        let subscription = statement
            .query_row(params![subscription_id], |row| {
                Self::subscription_from_row(row)
            })
            .optional()
            .map_err(db_error)?;
        Ok(subscription)
    }

    fn subscription_from_row(
        row: &rusqlite::Row<'_>,
    ) -> Result<NotificationSubscription, rusqlite::Error> {
        Ok(NotificationSubscription {
            subscription_id: row.get::<_, String>(0)?,
            recipient_id: row.get::<_, String>(1)?,
            transport_kind: Self::transport_kind_from_str(&row.get::<_, String>(2)?),
            endpoint: serde_json::from_str(&row.get::<_, String>(3)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
            enabled: row.get::<_, i64>(4)? != 0,
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(6)?)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
        })
    }

    fn load_delivery_job_tx(
        transaction: &rusqlite::Transaction<'_>,
        job_id: &str,
    ) -> TTResult<Option<NotificationDeliveryJob>> {
        let mut statement = transaction
            .prepare(
                "select job_id, origin_node_id, candidate_id, trigger_sequence, recipient_id, subscription_id, transport_kind, status, attempt_count, created_at, updated_at, dispatched_at, delivered_at, failed_at, suppressed_at, skipped_at, obsolete_at, receipt_json, error_text
                 from notification_delivery_jobs where job_id = ?1",
            )
            .map_err(db_error)?;
        let job = statement
            .query_row(params![job_id], |row| Self::job_from_row(row))
            .optional()
            .map_err(db_error)?;
        Ok(job)
    }

    fn job_from_row(row: &rusqlite::Row<'_>) -> Result<NotificationDeliveryJob, rusqlite::Error> {
        let parse_ts = |index: usize| -> Result<DateTime<Utc>, rusqlite::Error> {
            DateTime::parse_from_rfc3339(&row.get::<_, String>(index)?)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
        };
        let parse_opt_ts = |index: usize| -> Result<Option<DateTime<Utc>>, rusqlite::Error> {
            row.get::<_, Option<String>>(index)?
                .map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                index,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })
                })
                .transpose()
        };
        Ok(NotificationDeliveryJob {
            job_id: row.get::<_, String>(0)?,
            origin_node_id: row.get::<_, String>(1)?,
            candidate_id: row.get::<_, String>(2)?,
            trigger_sequence: row.get::<_, i64>(3)? as u64,
            recipient_id: row.get::<_, String>(4)?,
            subscription_id: row.get::<_, String>(5)?,
            transport_kind: Self::transport_kind_from_str(&row.get::<_, String>(6)?),
            status: Self::delivery_job_status_from_str(&row.get::<_, String>(7)?),
            attempt_count: row.get::<_, i64>(8)? as u64,
            created_at: parse_ts(9)?,
            updated_at: parse_ts(10)?,
            dispatched_at: parse_opt_ts(11)?,
            delivered_at: parse_opt_ts(12)?,
            failed_at: parse_opt_ts(13)?,
            suppressed_at: parse_opt_ts(14)?,
            skipped_at: parse_opt_ts(15)?,
            obsolete_at: parse_opt_ts(16)?,
            receipt: row
                .get::<_, Option<String>>(17)?
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        17,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            error: row.get::<_, Option<String>>(18)?,
        })
    }

    fn transport_kind_to_str(kind: NotificationTransportKind) -> &'static str {
        match kind {
            NotificationTransportKind::Log => "log",
            NotificationTransportKind::Mock => "mock",
            NotificationTransportKind::Webhook => "webhook",
            NotificationTransportKind::Apns => "apns",
            NotificationTransportKind::Fcm => "fcm",
            NotificationTransportKind::WebPush => "web_push",
        }
    }

    fn transport_kind_from_str(kind: &str) -> NotificationTransportKind {
        match kind {
            "mock" => NotificationTransportKind::Mock,
            "webhook" => NotificationTransportKind::Webhook,
            "apns" => NotificationTransportKind::Apns,
            "fcm" => NotificationTransportKind::Fcm,
            "web_push" => NotificationTransportKind::WebPush,
            _ => NotificationTransportKind::Log,
        }
    }

    fn delivery_job_status_to_str(status: NotificationDeliveryJobStatus) -> &'static str {
        match status {
            NotificationDeliveryJobStatus::Pending => "pending",
            NotificationDeliveryJobStatus::Dispatched => "dispatched",
            NotificationDeliveryJobStatus::Delivered => "delivered",
            NotificationDeliveryJobStatus::Failed => "failed",
            NotificationDeliveryJobStatus::Suppressed => "suppressed",
            NotificationDeliveryJobStatus::Skipped => "skipped",
            NotificationDeliveryJobStatus::Obsolete => "obsolete",
        }
    }

    fn delivery_job_status_from_str(status: &str) -> NotificationDeliveryJobStatus {
        match status {
            "dispatched" => NotificationDeliveryJobStatus::Dispatched,
            "delivered" => NotificationDeliveryJobStatus::Delivered,
            "failed" => NotificationDeliveryJobStatus::Failed,
            "suppressed" => NotificationDeliveryJobStatus::Suppressed,
            "skipped" => NotificationDeliveryJobStatus::Skipped,
            "obsolete" => NotificationDeliveryJobStatus::Obsolete,
            _ => NotificationDeliveryJobStatus::Pending,
        }
    }

    fn delivery_job_id(
        origin_node_id: &str,
        candidate_id: &str,
        subscription_id: &str,
        trigger_sequence: u64,
    ) -> String {
        format!("{origin_node_id}::{candidate_id}::{subscription_id}::{trigger_sequence}")
    }

    fn enqueue_delivery_jobs_for_recipient_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        recipient_id: &str,
        now: DateTime<Utc>,
    ) -> TTResult<()> {
        let mut statement = transaction
            .prepare(
                "select subscription_id from notification_subscriptions where recipient_id = ?1 and enabled = 1",
            )
            .map_err(db_error)?;
        let mut rows = statement.query(params![recipient_id]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let subscription_id = row.get::<_, String>(0).map_err(db_error)?;
            self.enqueue_delivery_jobs_for_subscription_tx(transaction, &subscription_id, now)?;
        }
        Ok(())
    }

    fn enqueue_delivery_jobs_for_candidate_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        candidate: &OperatorNotificationCandidate,
        now: DateTime<Utc>,
    ) -> TTResult<()> {
        if candidate.status != OperatorNotificationCandidateStatus::Pending {
            return Ok(());
        }
        let mut statement = transaction
            .prepare(
                "select subscription_id, recipient_id, transport_kind, endpoint_json, enabled, created_at, updated_at
                 from notification_subscriptions
                 where enabled = 1
                 order by subscription_id asc",
            )
            .map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let subscription = Self::subscription_from_row(row).map_err(db_error)?;
            let recipient =
                Self::load_recipient_tx(transaction, subscription.recipient_id.as_str())?
                    .ok_or_else(|| {
                        TTError::Store("notification recipient not found".to_string())
                    })?;
            if !recipient.enabled {
                continue;
            }
            self.upsert_delivery_job_for_candidate_tx(
                transaction,
                candidate,
                &recipient,
                &subscription,
                now,
            )?;
        }
        Ok(())
    }

    fn enqueue_delivery_jobs_for_subscription_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        subscription_id: &str,
        now: DateTime<Utc>,
    ) -> TTResult<()> {
        let subscription = Self::load_subscription_tx(transaction, subscription_id)?
            .ok_or_else(|| TTError::Store("notification subscription not found".to_string()))?;
        if !subscription.enabled {
            return Ok(());
        }
        let recipient =
            Self::load_recipient_tx(transaction, subscription.recipient_id.as_str())?
                .ok_or_else(|| TTError::Store("notification recipient not found".to_string()))?;
        if !recipient.enabled {
            return Ok(());
        }
        let mut statement = transaction
            .prepare(
                "select candidate_id, origin_node_id, item_id, trigger_sequence, candidate_status, item_json, created_at, updated_at, acknowledged_at, suppressed_at, resolved_at, obsolete_at
                 from mirrored_notification_candidates
                 where candidate_status = ?1",
            )
            .map_err(db_error)?;
        let mut rows = statement
            .query(params![Self::candidate_status_to_str(
                OperatorNotificationCandidateStatus::Pending
            )])
            .map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let candidate = Self::candidate_from_row(row).map_err(db_error)?;
            self.upsert_delivery_job_for_candidate_tx(
                transaction,
                &candidate,
                &recipient,
                &subscription,
                now,
            )?;
        }
        Ok(())
    }

    fn disable_delivery_jobs_for_recipient_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        recipient_id: &str,
        now: DateTime<Utc>,
    ) -> TTResult<()> {
        transaction
            .execute(
                "update notification_delivery_jobs
                 set status = ?2,
                     updated_at = ?3,
                     suppressed_at = coalesce(suppressed_at, ?3)
                 where recipient_id = ?1
                   and status in (?4, ?5, ?6, ?7)",
                params![
                    recipient_id,
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Suppressed),
                    now.to_rfc3339(),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Pending),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Dispatched),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Failed),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Skipped),
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn disable_delivery_jobs_for_subscription_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        subscription_id: &str,
        now: DateTime<Utc>,
    ) -> TTResult<()> {
        transaction
            .execute(
                "update notification_delivery_jobs
                 set status = ?2,
                     updated_at = ?3,
                     suppressed_at = coalesce(suppressed_at, ?3)
                 where subscription_id = ?1
                   and status in (?4, ?5, ?6, ?7)",
                params![
                    subscription_id,
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Suppressed),
                    now.to_rfc3339(),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Pending),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Dispatched),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Failed),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Skipped),
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn mark_delivery_jobs_for_candidate_status_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        candidate_id: &str,
        status: NotificationDeliveryJobStatus,
        now: DateTime<Utc>,
    ) -> TTResult<()> {
        let timestamp_column = match status {
            NotificationDeliveryJobStatus::Suppressed => Some("suppressed_at"),
            NotificationDeliveryJobStatus::Obsolete => Some("obsolete_at"),
            _ => None,
        };
        let Some(timestamp_column) = timestamp_column else {
            return Ok(());
        };
        let sql_status = Self::delivery_job_status_to_str(status);
        let sql = format!(
            "update notification_delivery_jobs
             set status = ?2,
                 updated_at = ?3,
                 {timestamp_column} = coalesce({timestamp_column}, ?3)
             where candidate_id = ?1
               and status in (?4, ?5, ?6, ?7)"
        );
        transaction
            .execute(
                sql.as_str(),
                params![
                    candidate_id,
                    sql_status,
                    now.to_rfc3339(),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Pending),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Dispatched),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Failed),
                    Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Skipped),
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn upsert_delivery_job_for_candidate_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        candidate: &OperatorNotificationCandidate,
        recipient: &NotificationRecipient,
        subscription: &NotificationSubscription,
        now: DateTime<Utc>,
    ) -> TTResult<()> {
        if candidate.status != OperatorNotificationCandidateStatus::Pending {
            return Ok(());
        }
        let job_id = Self::delivery_job_id(
            candidate.origin_node_id.as_str(),
            candidate.candidate_id.as_str(),
            subscription.subscription_id.as_str(),
            candidate.trigger_sequence,
        );
        let existing = Self::load_delivery_job_tx(transaction, job_id.as_str())?;
        match existing {
            Some(job)
                if matches!(
                    job.status,
                    NotificationDeliveryJobStatus::Pending | NotificationDeliveryJobStatus::Skipped
                ) =>
            {
                transaction
                    .execute(
                        "update notification_delivery_jobs
                     set status = ?3,
                         updated_at = ?4,
                         recipient_id = ?5,
                         transport_kind = ?6,
                         dispatched_at = null,
                         delivered_at = null,
                         failed_at = null,
                         suppressed_at = null,
                         skipped_at = null,
                         obsolete_at = null,
                         error_text = null,
                         receipt_json = null
                     where job_id = ?1 and candidate_id = ?2",
                        params![
                            job_id,
                            candidate.candidate_id.as_str(),
                            Self::delivery_job_status_to_str(
                                NotificationDeliveryJobStatus::Pending
                            ),
                            now.to_rfc3339(),
                            recipient.recipient_id.as_str(),
                            Self::transport_kind_to_str(subscription.transport_kind),
                        ],
                    )
                    .map_err(db_error)?;
            }
            Some(_) => {}
            None => {
                transaction.execute(
            "insert into notification_delivery_jobs(job_id, origin_node_id, candidate_id, trigger_sequence, recipient_id, subscription_id, transport_kind, status, attempt_count, created_at, updated_at, dispatched_at, delivered_at, failed_at, suppressed_at, skipped_at, obsolete_at, receipt_json, error_text)
                     values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10, null, null, null, null, null, null, null, null)",
                    params![
                        job_id,
                        candidate.origin_node_id.as_str(),
                        candidate.candidate_id.as_str(),
                        candidate.trigger_sequence as i64,
                        recipient.recipient_id.as_str(),
                        subscription.subscription_id.as_str(),
                        Self::transport_kind_to_str(subscription.transport_kind),
                        Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Pending),
                        now.to_rfc3339(),
                        now.to_rfc3339(),
                    ],
                ).map_err(db_error)?;
            }
        }
        Ok(())
    }

    fn remote_action_request_status_to_str(
        status: OperatorRemoteActionRequestStatus,
    ) -> &'static str {
        match status {
            OperatorRemoteActionRequestStatus::Pending => "pending",
            OperatorRemoteActionRequestStatus::Claimed => "claimed",
            OperatorRemoteActionRequestStatus::Completed => "completed",
            OperatorRemoteActionRequestStatus::Failed => "failed",
            OperatorRemoteActionRequestStatus::Canceled => "canceled",
            OperatorRemoteActionRequestStatus::Stale => "stale",
        }
    }

    fn remote_action_request_status_from_str(status: &str) -> OperatorRemoteActionRequestStatus {
        match status {
            "claimed" => OperatorRemoteActionRequestStatus::Claimed,
            "completed" => OperatorRemoteActionRequestStatus::Completed,
            "failed" => OperatorRemoteActionRequestStatus::Failed,
            "canceled" => OperatorRemoteActionRequestStatus::Canceled,
            "stale" => OperatorRemoteActionRequestStatus::Stale,
            _ => OperatorRemoteActionRequestStatus::Pending,
        }
    }

    fn remote_action_request_id(
        origin_node_id: &str,
        candidate_id: &str,
        action_kind: tt_core::ipc::OperatorInboxActionKind,
        idempotency_key: Option<&str>,
    ) -> String {
        match idempotency_key {
            Some(key) if !key.is_empty() => format!(
                "{origin_node_id}::{candidate_id}::{}::{key}",
                Self::operator_inbox_action_kind_to_str(action_kind)
            ),
            _ => format!(
                "{origin_node_id}::{candidate_id}::{}",
                Self::operator_inbox_action_kind_to_str(action_kind)
            ),
        }
    }

    fn operator_inbox_action_kind_to_str(
        action_kind: tt_core::ipc::OperatorInboxActionKind,
    ) -> &'static str {
        match action_kind {
            tt_core::ipc::OperatorInboxActionKind::Approve => "approve",
            tt_core::ipc::OperatorInboxActionKind::Reject => "reject",
            tt_core::ipc::OperatorInboxActionKind::ApproveAndSend => "approve_and_send",
            tt_core::ipc::OperatorInboxActionKind::RecordNoAction => "record_no_action",
            tt_core::ipc::OperatorInboxActionKind::ManualRefresh => "manual_refresh",
            tt_core::ipc::OperatorInboxActionKind::Reconcile => "reconcile",
            tt_core::ipc::OperatorInboxActionKind::Retry => "retry",
            tt_core::ipc::OperatorInboxActionKind::Supersede => "supersede",
            tt_core::ipc::OperatorInboxActionKind::MarkReadyForReview => "mark_ready_for_review",
        }
    }

    fn operator_inbox_action_kind_from_str(
        action_kind: &str,
    ) -> tt_core::ipc::OperatorInboxActionKind {
        match action_kind {
            "reject" => tt_core::ipc::OperatorInboxActionKind::Reject,
            "approve_and_send" => tt_core::ipc::OperatorInboxActionKind::ApproveAndSend,
            "record_no_action" => tt_core::ipc::OperatorInboxActionKind::RecordNoAction,
            "manual_refresh" => tt_core::ipc::OperatorInboxActionKind::ManualRefresh,
            "reconcile" => tt_core::ipc::OperatorInboxActionKind::Reconcile,
            "retry" => tt_core::ipc::OperatorInboxActionKind::Retry,
            "supersede" => tt_core::ipc::OperatorInboxActionKind::Supersede,
            "mark_ready_for_review" => tt_core::ipc::OperatorInboxActionKind::MarkReadyForReview,
            _ => tt_core::ipc::OperatorInboxActionKind::Approve,
        }
    }

    fn remote_action_request_from_row(
        row: &rusqlite::Row<'_>,
    ) -> Result<OperatorRemoteActionRequest, rusqlite::Error> {
        let column_count = row.as_ref().column_count();
        let parse_ts = |index: usize| -> Result<DateTime<Utc>, rusqlite::Error> {
            DateTime::parse_from_rfc3339(&row.get::<_, String>(index)?)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
        };
        let parse_opt_ts = |index: usize| -> Result<Option<DateTime<Utc>>, rusqlite::Error> {
            row.get::<_, Option<String>>(index)?
                .map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                index,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })
                })
                .transpose()
        };
        let (
            claim_token,
            completed_at_index,
            failed_at_index,
            canceled_at_index,
            stale_at_index,
            attempt_count_index,
            result_index,
            error_index,
        ) = if column_count >= 24 {
            (
                row.get::<_, Option<String>>(16)?,
                17,
                18,
                19,
                20,
                21,
                22,
                23,
            )
        } else if column_count == 23 {
            (
                row.get::<_, Option<String>>(15)?,
                16,
                17,
                18,
                19,
                20,
                21,
                22,
            )
        } else if column_count == 22 {
            (None, 15, 16, 17, 18, 19, 20, 21)
        } else {
            (
                row.get::<_, Option<String>>(15)?,
                16,
                17,
                18,
                19,
                20,
                21,
                22,
            )
        };
        Ok(OperatorRemoteActionRequest {
            request_id: row.get::<_, String>(0)?,
            origin_node_id: row.get::<_, String>(1)?,
            candidate_id: row.get::<_, String>(2)?,
            item_id: row.get::<_, String>(3)?,
            trigger_sequence: row.get::<_, i64>(4)? as u64,
            action_kind: Self::operator_inbox_action_kind_from_str(&row.get::<_, String>(5)?),
            idempotency_key: row.get::<_, Option<String>>(6)?,
            item: serde_json::from_str(&row.get::<_, String>(7)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
            requested_by: row.get::<_, Option<String>>(8)?,
            request_note: row.get::<_, Option<String>>(9)?,
            status: Self::remote_action_request_status_from_str(&row.get::<_, String>(10)?),
            created_at: parse_ts(11)?,
            updated_at: parse_ts(12)?,
            claimed_by: row.get::<_, Option<String>>(13)?,
            claimed_at: parse_opt_ts(14)?,
            claimed_until: parse_opt_ts(15)?,
            claim_token,
            completed_at: parse_opt_ts(completed_at_index)?,
            failed_at: parse_opt_ts(failed_at_index)?,
            canceled_at: parse_opt_ts(canceled_at_index)?,
            stale_at: parse_opt_ts(stale_at_index)?,
            attempt_count: row.get::<_, i64>(attempt_count_index)? as u64,
            result: row
                .get::<_, Option<String>>(result_index)?
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        result_index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            error: row.get::<_, Option<String>>(error_index)?,
        })
    }

    fn remote_action_item_is_actionable(item: &OperatorInboxItem) -> bool {
        item.status == tt_core::ipc::OperatorInboxItemStatus::Open
            && !item.available_actions.is_empty()
    }

    fn load_remote_action_request_tx(
        transaction: &rusqlite::Transaction<'_>,
        request_id: &str,
    ) -> TTResult<Option<OperatorRemoteActionRequest>> {
        let mut statement = transaction
            .prepare(
                "select request_id, origin_node_id, candidate_id, item_id, trigger_sequence, action_kind, idempotency_key, item_json, requested_by, request_note, request_status, created_at, updated_at, claimed_by, claimed_at, claimed_until, claim_token, completed_at, failed_at, canceled_at, stale_at, attempt_count, result_json, error_text
                 from remote_action_requests where request_id = ?1",
            )
            .map_err(db_error)?;
        let request = statement
            .query_row(params![request_id], |row| {
                Self::remote_action_request_from_row(row)
            })
            .optional()
            .map_err(db_error)?;
        Ok(request)
    }

    fn update_remote_action_request_status_tx(
        transaction: &rusqlite::Transaction<'_>,
        request_id: &str,
        status: OperatorRemoteActionRequestStatus,
        updated_at: DateTime<Utc>,
        attempt_count_delta: i64,
        claimed_by: Option<&str>,
        claimed_at: Option<DateTime<Utc>>,
        claimed_until: Option<DateTime<Utc>>,
        claim_token: Option<&str>,
        completed_at: Option<DateTime<Utc>>,
        failed_at: Option<DateTime<Utc>>,
        canceled_at: Option<DateTime<Utc>>,
        stale_at: Option<DateTime<Utc>>,
        result: Option<Value>,
        error: Option<String>,
    ) -> TTResult<()> {
        transaction
            .execute(
                "update remote_action_requests
                 set request_status = ?2,
                     updated_at = ?3,
                     attempt_count = attempt_count + ?4,
                     claimed_by = coalesce(?5, claimed_by),
                     claimed_at = coalesce(?6, claimed_at),
                     claimed_until = coalesce(?7, claimed_until),
                     claim_token = coalesce(?8, claim_token),
                     completed_at = coalesce(?9, completed_at),
                     failed_at = coalesce(?10, failed_at),
                     canceled_at = coalesce(?11, canceled_at),
                     stale_at = coalesce(?12, stale_at),
                     result_json = coalesce(?13, result_json),
                     error_text = coalesce(?14, error_text)
                 where request_id = ?1",
                params![
                    request_id,
                    Self::remote_action_request_status_to_str(status),
                    updated_at.to_rfc3339(),
                    attempt_count_delta,
                    claimed_by,
                    claimed_at.map(|value| value.to_rfc3339()),
                    claimed_until.map(|value| value.to_rfc3339()),
                    claim_token,
                    completed_at.map(|value| value.to_rfc3339()),
                    failed_at.map(|value| value.to_rfc3339()),
                    canceled_at.map(|value| value.to_rfc3339()),
                    stale_at.map(|value| value.to_rfc3339()),
                    result.map(|value| value.to_string()),
                    error,
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn mark_remote_action_requests_for_candidate_status_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        candidate_id: &str,
        status: OperatorRemoteActionRequestStatus,
        now: DateTime<Utc>,
    ) -> TTResult<()> {
        if status != OperatorRemoteActionRequestStatus::Stale {
            return Ok(());
        }
        transaction
            .execute(
                "update remote_action_requests
                 set request_status = ?2,
                     updated_at = ?3,
                     stale_at = coalesce(stale_at, ?3)
                 where candidate_id = ?1
                   and request_status in (?4, ?5)",
                params![
                    candidate_id,
                    Self::remote_action_request_status_to_str(status),
                    now.to_rfc3339(),
                    Self::remote_action_request_status_to_str(
                        OperatorRemoteActionRequestStatus::Pending
                    ),
                    Self::remote_action_request_status_to_str(
                        OperatorRemoteActionRequestStatus::Claimed
                    ),
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn update_delivery_job_status_tx(
        transaction: &rusqlite::Transaction<'_>,
        job_id: &str,
        status: NotificationDeliveryJobStatus,
        updated_at: DateTime<Utc>,
        attempt_count_delta: i64,
        dispatched_at: Option<DateTime<Utc>>,
        delivered_at: Option<DateTime<Utc>>,
        failed_at: Option<DateTime<Utc>>,
        suppressed_at: Option<DateTime<Utc>>,
        skipped_at: Option<DateTime<Utc>>,
        obsolete_at: Option<DateTime<Utc>>,
        receipt: Option<Value>,
        error: Option<String>,
    ) -> TTResult<()> {
        transaction
            .execute(
                "update notification_delivery_jobs
                 set status = ?2,
                     attempt_count = attempt_count + ?3,
                     updated_at = ?4,
                     dispatched_at = coalesce(?5, dispatched_at),
                     delivered_at = coalesce(?6, delivered_at),
                     failed_at = coalesce(?7, failed_at),
                     suppressed_at = coalesce(?8, suppressed_at),
                     skipped_at = coalesce(?9, skipped_at),
                     obsolete_at = coalesce(?10, obsolete_at),
                     receipt_json = coalesce(?11, receipt_json),
                     error_text = coalesce(?12, error_text)
                 where job_id = ?1",
                params![
                    job_id,
                    Self::delivery_job_status_to_str(status),
                    attempt_count_delta,
                    updated_at.to_rfc3339(),
                    dispatched_at.map(|value| value.to_rfc3339()),
                    delivered_at.map(|value| value.to_rfc3339()),
                    failed_at.map(|value| value.to_rfc3339()),
                    suppressed_at.map(|value| value.to_rfc3339()),
                    skipped_at.map(|value| value.to_rfc3339()),
                    obsolete_at.map(|value| value.to_rfc3339()),
                    receipt.map(|value| value.to_string()),
                    error,
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    fn dispatch_job_tx<T: NotificationDeliveryTransport + ?Sized>(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        transport: &T,
        job: NotificationDeliveryJob,
        candidate: Option<OperatorNotificationCandidate>,
        subscription: Option<NotificationSubscription>,
        recipient: Option<NotificationRecipient>,
        now: DateTime<Utc>,
    ) -> TTResult<NotificationDeliveryJob> {
        let candidate = match candidate {
            Some(candidate) => candidate,
            None => {
                Self::update_delivery_job_status_tx(
                    transaction,
                    job.job_id.as_str(),
                    NotificationDeliveryJobStatus::Obsolete,
                    now,
                    0,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(now),
                    None,
                    Some("mirrored notification candidate missing".to_string()),
                )?;
                return Self::load_delivery_job_tx(transaction, job.job_id.as_str())?.ok_or_else(
                    || TTError::Store("delivery job disappeared during update".to_string()),
                );
            }
        };
        let subscription = match subscription {
            Some(subscription) if subscription.enabled => subscription,
            _ => {
                Self::update_delivery_job_status_tx(
                    transaction,
                    job.job_id.as_str(),
                    NotificationDeliveryJobStatus::Skipped,
                    now,
                    0,
                    None,
                    None,
                    None,
                    None,
                    Some(now),
                    None,
                    None,
                    Some("notification subscription unavailable or disabled".to_string()),
                )?;
                return Self::load_delivery_job_tx(transaction, job.job_id.as_str())?.ok_or_else(
                    || TTError::Store("delivery job disappeared during update".to_string()),
                );
            }
        };
        let recipient = match recipient {
            Some(recipient) if recipient.enabled => recipient,
            _ => {
                Self::update_delivery_job_status_tx(
                    transaction,
                    job.job_id.as_str(),
                    NotificationDeliveryJobStatus::Skipped,
                    now,
                    0,
                    None,
                    None,
                    None,
                    None,
                    Some(now),
                    None,
                    None,
                    Some("notification recipient unavailable or disabled".to_string()),
                )?;
                return Self::load_delivery_job_tx(transaction, job.job_id.as_str())?.ok_or_else(
                    || TTError::Store("delivery job disappeared during update".to_string()),
                );
            }
        };
        match candidate.status {
            OperatorNotificationCandidateStatus::Pending => {}
            OperatorNotificationCandidateStatus::Acknowledged
            | OperatorNotificationCandidateStatus::Suppressed => {
                Self::update_delivery_job_status_tx(
                    transaction,
                    job.job_id.as_str(),
                    NotificationDeliveryJobStatus::Suppressed,
                    now,
                    0,
                    None,
                    None,
                    None,
                    Some(now),
                    None,
                    None,
                    None,
                    Some("notification candidate was acknowledged or suppressed".to_string()),
                )?;
                return Self::load_delivery_job_tx(transaction, job.job_id.as_str())?.ok_or_else(
                    || TTError::Store("delivery job disappeared during update".to_string()),
                );
            }
            OperatorNotificationCandidateStatus::Obsolete => {
                Self::update_delivery_job_status_tx(
                    transaction,
                    job.job_id.as_str(),
                    NotificationDeliveryJobStatus::Obsolete,
                    now,
                    0,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(now),
                    None,
                    Some("notification candidate is obsolete".to_string()),
                )?;
                return Self::load_delivery_job_tx(transaction, job.job_id.as_str())?.ok_or_else(
                    || TTError::Store("delivery job disappeared during update".to_string()),
                );
            }
        }

        Self::update_delivery_job_status_tx(
            transaction,
            job.job_id.as_str(),
            NotificationDeliveryJobStatus::Dispatched,
            now,
            1,
            Some(now),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )?;
        let updated_job = Self::load_delivery_job_tx(transaction, job.job_id.as_str())?
            .ok_or_else(|| TTError::Store("delivery job disappeared during update".to_string()))?;
        let context = NotificationDeliveryContext {
            job: &updated_job,
            candidate: &candidate,
            recipient: &recipient,
            subscription: &subscription,
        };
        let outcome = transport.dispatch(&context);
        let final_status = match outcome.status {
            NotificationDeliveryJobStatus::Delivered
            | NotificationDeliveryJobStatus::Failed
            | NotificationDeliveryJobStatus::Suppressed
            | NotificationDeliveryJobStatus::Skipped
            | NotificationDeliveryJobStatus::Obsolete => outcome.status,
            NotificationDeliveryJobStatus::Pending | NotificationDeliveryJobStatus::Dispatched => {
                NotificationDeliveryJobStatus::Failed
            }
        };
        let now = Utc::now();
        Self::update_delivery_job_status_tx(
            transaction,
            job.job_id.as_str(),
            final_status,
            now,
            0,
            None,
            if final_status == NotificationDeliveryJobStatus::Delivered {
                Some(now)
            } else {
                None
            },
            if final_status == NotificationDeliveryJobStatus::Failed {
                Some(now)
            } else {
                None
            },
            if final_status == NotificationDeliveryJobStatus::Suppressed {
                Some(now)
            } else {
                None
            },
            if final_status == NotificationDeliveryJobStatus::Skipped {
                Some(now)
            } else {
                None
            },
            if final_status == NotificationDeliveryJobStatus::Obsolete {
                Some(now)
            } else {
                None
            },
            outcome.receipt,
            outcome.error,
        )?;
        Self::load_delivery_job_tx(transaction, job.job_id.as_str())?
            .ok_or_else(|| TTError::Store("delivery job disappeared during update".to_string()))
    }

    pub fn list(
        &self,
        origin_node_id: &str,
        limit: Option<usize>,
    ) -> TTResult<OperatorInboxMirrorListResponse> {
        let checkpoint = self.checkpoint(origin_node_id)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection.prepare(
            "select item_json from mirrored_inbox_items where origin_node_id = ?1 order by changed_at desc, item_id asc",
        ).map_err(db_error)?;
        let mut rows = statement.query(params![origin_node_id]).map_err(db_error)?;
        let mut items = Vec::new();
        while let Some(row) = rows.next().map_err(db_error)? {
            let item_json = row.get::<_, String>(0).map_err(db_error)?;
            let item: OperatorInboxItem = serde_json::from_str(&item_json)?;
            items.push(item);
        }
        if let Some(limit) = limit {
            items.truncate(limit);
        }
        Ok(OperatorInboxMirrorListResponse {
            origin_node_id: origin_node_id.to_string(),
            checkpoint,
            items,
        })
    }

    pub fn get(&self, origin_node_id: &str, item_id: &str) -> TTResult<Option<OperatorInboxItem>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection.prepare(
            "select item_json from mirrored_inbox_items where origin_node_id = ?1 and item_id = ?2",
        ).map_err(db_error)?;
        let item = statement
            .query_row(params![origin_node_id, item_id], |row| {
                let item_json = row.get::<_, String>(0)?;
                serde_json::from_str::<OperatorInboxItem>(&item_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .optional()
            .map_err(db_error)?;
        Ok(item)
    }

    pub fn apply_batch(
        &self,
        origin_node_id: &str,
        _source_checkpoint: OperatorInboxCheckpoint,
        changes: &[OperatorInboxChange],
    ) -> TTResult<MirrorApplyResult> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let mut checkpoint = self.load_checkpoint_tx(&transaction, origin_node_id)?;
        let mut applied_changes = 0usize;
        let mut skipped_changes = 0usize;

        for change in changes {
            if change.sequence <= checkpoint.current_sequence {
                skipped_changes += 1;
                continue;
            }
            let expected = checkpoint.current_sequence + 1;
            if change.sequence != expected {
                return Err(TTError::Store(format!(
                    "inbox mirror batch for origin `{origin_node_id}` is missing sequence {expected} before {}",
                    change.sequence
                )));
            }
            let previous_item =
                Self::load_mirrored_item_tx(&transaction, origin_node_id, change.item.id.as_str())?;
            match change.kind {
                tt_core::ipc::OperatorInboxChangeKind::Upsert => {
                    let item_json = serde_json::to_string(&change.item)?;
                    transaction.execute(
                        "insert into mirrored_inbox_items(origin_node_id, item_id, sequence, item_json, changed_at)
                         values(?1, ?2, ?3, ?4, ?5)
                         on conflict(origin_node_id, item_id) do update set
                           sequence = excluded.sequence,
                           item_json = excluded.item_json,
                           changed_at = excluded.changed_at",
                        params![
                            origin_node_id,
                            change.item.id.as_str(),
                            change.sequence as i64,
                            item_json,
                            change.changed_at.to_rfc3339(),
                        ],
                    ).map_err(db_error)?;
                    self.apply_notification_transition_tx(
                        &transaction,
                        origin_node_id,
                        previous_item.as_ref(),
                        Some(&change.item),
                        change.sequence,
                        change.changed_at,
                    )?;
                }
                tt_core::ipc::OperatorInboxChangeKind::Removed => {
                    transaction.execute(
                        "delete from mirrored_inbox_items where origin_node_id = ?1 and item_id = ?2",
                        params![origin_node_id, change.item.id.as_str()],
                    ).map_err(db_error)?;
                    self.apply_notification_transition_tx(
                        &transaction,
                        origin_node_id,
                        previous_item.as_ref(),
                        None,
                        change.sequence,
                        change.changed_at,
                    )?;
                }
            }
            checkpoint.current_sequence = change.sequence;
            checkpoint.updated_at = change.changed_at;
            applied_changes += 1;
        }

        transaction.execute(
            "insert into mirrored_inbox_checkpoint(origin_node_id, current_sequence, updated_at)
             values(?1, ?2, ?3)
             on conflict(origin_node_id) do update set
               current_sequence = excluded.current_sequence,
               updated_at = excluded.updated_at",
            params![
                origin_node_id,
                checkpoint.current_sequence as i64,
                checkpoint.updated_at.to_rfc3339(),
            ],
        ).map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        let _ = self.checkpoint_events.send(());

        Ok(MirrorApplyResult {
            mirror_checkpoint: OperatorInboxMirrorCheckpoint {
                peer_id: origin_node_id.to_string(),
                last_exported_sequence: checkpoint.current_sequence,
                last_acked_sequence: checkpoint.current_sequence,
                updated_at: checkpoint.updated_at,
            },
            checkpoint,
            applied_changes,
            skipped_changes,
        })
    }

    pub fn notification_candidates(
        &self,
        request: &OperatorNotificationListRequest,
    ) -> TTResult<OperatorNotificationListResponse> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection.prepare(
            "select candidate_id, origin_node_id, item_id, trigger_sequence, candidate_status, item_json, created_at, updated_at, acknowledged_at, suppressed_at, resolved_at, obsolete_at
             from mirrored_notification_candidates
             where origin_node_id = ?1
             order by updated_at desc, candidate_id asc",
        ).map_err(db_error)?;
        let mut rows = statement
            .query(params![request.origin_node_id.as_str()])
            .map_err(db_error)?;
        let mut candidates = Vec::new();
        while let Some(row) = rows.next().map_err(db_error)? {
            let candidate = Self::candidate_from_row(row).map_err(db_error)?;
            if request.pending_only
                && candidate.status != OperatorNotificationCandidateStatus::Pending
            {
                continue;
            }
            if let Some(status) = request.status {
                if candidate.status != status {
                    continue;
                }
            }
            if request.actionable_only && !Self::candidate_is_actionable(&candidate) {
                continue;
            }
            candidates.push(candidate);
        }
        if let Some(limit) = request.limit {
            candidates.truncate(limit);
        }
        Ok(OperatorNotificationListResponse {
            origin_node_id: request.origin_node_id.clone(),
            candidates,
        })
    }

    pub fn notification_candidate(
        &self,
        request: &OperatorNotificationGetRequest,
    ) -> TTResult<Option<OperatorNotificationCandidate>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection.prepare(
            "select candidate_id, origin_node_id, item_id, trigger_sequence, candidate_status, item_json, created_at, updated_at, acknowledged_at, suppressed_at, resolved_at, obsolete_at
             from mirrored_notification_candidates
             where origin_node_id = ?1 and candidate_id = ?2",
        ).map_err(db_error)?;
        let candidate = statement
            .query_row(
                params![
                    request.origin_node_id.as_str(),
                    request.candidate_id.as_str()
                ],
                |row| Self::candidate_from_row(row),
            )
            .optional()
            .map_err(db_error)?;
        Ok(candidate)
    }

    pub fn acknowledge_notification_candidate(
        &self,
        request: &OperatorNotificationAckRequest,
    ) -> TTResult<OperatorNotificationAckResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let candidate = Self::load_notification_candidate_tx(
            &transaction,
            request.origin_node_id.as_str(),
            request.candidate_id.as_str(),
        )?
        .ok_or_else(|| TTError::Store("notification candidate not found".to_string()))?;
        let next = match candidate.status {
            OperatorNotificationCandidateStatus::Pending => {
                let updated_at = Utc::now();
                self.write_notification_candidate_status_tx(
                    &transaction,
                    request.origin_node_id.as_str(),
                    request.candidate_id.as_str(),
                    OperatorNotificationCandidateStatus::Acknowledged,
                    updated_at,
                    Some(updated_at),
                    candidate.suppressed_at,
                    candidate.resolved_at,
                    None,
                )?
            }
            OperatorNotificationCandidateStatus::Acknowledged => candidate,
            OperatorNotificationCandidateStatus::Suppressed => {
                return Err(TTError::Store(
                    "suppressed notification candidates cannot be acknowledged".to_string(),
                ));
            }
            OperatorNotificationCandidateStatus::Obsolete => {
                return Err(TTError::Store(
                    "obsolete notification candidates cannot be acknowledged".to_string(),
                ));
            }
        };
        self.mark_delivery_jobs_for_candidate_status_tx(
            &transaction,
            request.candidate_id.as_str(),
            NotificationDeliveryJobStatus::Suppressed,
            next.updated_at,
        )?;
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(OperatorNotificationAckResponse { candidate: next })
    }

    pub fn suppress_notification_candidate(
        &self,
        request: &OperatorNotificationSuppressRequest,
    ) -> TTResult<OperatorNotificationSuppressResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let candidate = Self::load_notification_candidate_tx(
            &transaction,
            request.origin_node_id.as_str(),
            request.candidate_id.as_str(),
        )?
        .ok_or_else(|| TTError::Store("notification candidate not found".to_string()))?;
        let next = match candidate.status {
            OperatorNotificationCandidateStatus::Pending
            | OperatorNotificationCandidateStatus::Acknowledged => {
                let updated_at = Utc::now();
                self.write_notification_candidate_status_tx(
                    &transaction,
                    request.origin_node_id.as_str(),
                    request.candidate_id.as_str(),
                    OperatorNotificationCandidateStatus::Suppressed,
                    updated_at,
                    candidate.acknowledged_at,
                    Some(updated_at),
                    candidate.resolved_at,
                    None,
                )?
            }
            OperatorNotificationCandidateStatus::Suppressed => candidate,
            OperatorNotificationCandidateStatus::Obsolete => {
                return Err(TTError::Store(
                    "obsolete notification candidates cannot be suppressed".to_string(),
                ));
            }
        };
        self.mark_delivery_jobs_for_candidate_status_tx(
            &transaction,
            request.candidate_id.as_str(),
            NotificationDeliveryJobStatus::Suppressed,
            next.updated_at,
        )?;
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(OperatorNotificationSuppressResponse { candidate: next })
    }

    pub fn create_remote_action_request(
        &self,
        request: &OperatorRemoteActionCreateRequest,
    ) -> TTResult<OperatorRemoteActionCreateResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let window = Self::load_notification_window_tx(
            &transaction,
            request.origin_node_id.as_str(),
            request.item_id.as_str(),
        )?
        .ok_or_else(|| {
            TTError::Store("no actionable notification window found for item".to_string())
        })?;
        let candidate = Self::load_notification_candidate_tx(
            &transaction,
            request.origin_node_id.as_str(),
            window.0.as_str(),
        )?
        .ok_or_else(|| TTError::Store("notification candidate not found".to_string()))?;
        if candidate.item.id != request.item_id {
            return Err(TTError::Store(
                "notification candidate does not match the requested item".to_string(),
            ));
        }
        if !candidate
            .item
            .available_actions
            .contains(&request.action_kind)
        {
            return Err(TTError::Store(format!(
                "action `{:?}` is not available for mirrored inbox item `{}`",
                request.action_kind, request.item_id
            )));
        }
        if !Self::remote_action_item_is_actionable(&candidate.item) {
            return Err(TTError::Store(format!(
                "mirrored inbox item `{}` is not actionable",
                request.item_id
            )));
        }

        let request_id = Self::remote_action_request_id(
            request.origin_node_id.as_str(),
            candidate.candidate_id.as_str(),
            request.action_kind,
            request.idempotency_key.as_deref(),
        );
        if let Some(existing) =
            Self::load_remote_action_request_tx(&transaction, request_id.as_str())?
        {
            transaction.commit().map_err(db_error)?;
            return Ok(OperatorRemoteActionCreateResponse { request: existing });
        }

        let now = Utc::now();
        let item_json = serde_json::to_string(&candidate.item)?;
        transaction
            .execute(
                "insert into remote_action_requests(request_id, origin_node_id, candidate_id, item_id, trigger_sequence, action_kind, idempotency_key, item_json, requested_by, request_note, request_status, created_at, updated_at, claimed_by, claimed_at, claimed_until, claim_token, completed_at, failed_at, canceled_at, stale_at, attempt_count, result_json, error_text)
                 values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, null, null, null, null, null, null, null, null, 0, null, null)",
                params![
                    request_id,
                    request.origin_node_id.as_str(),
                    candidate.candidate_id.as_str(),
                    request.item_id.as_str(),
                    candidate.trigger_sequence as i64,
                    Self::operator_inbox_action_kind_to_str(request.action_kind),
                    request.idempotency_key.as_deref(),
                    item_json,
                    request.requested_by.as_deref(),
                    request.request_note.as_deref(),
                    Self::remote_action_request_status_to_str(
                        OperatorRemoteActionRequestStatus::Pending
                    ),
                    now.to_rfc3339(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(db_error)?;
        let request = Self::load_remote_action_request_tx(&transaction, request_id.as_str())?
            .ok_or_else(|| {
                TTError::Store("remote action request disappeared after insert".to_string())
            })?;
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(OperatorRemoteActionCreateResponse { request })
    }

    pub fn list_remote_action_requests(
        &self,
        request: &OperatorRemoteActionListRequest,
    ) -> TTResult<OperatorRemoteActionListResponse> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection
            .prepare(
                "select request_id, origin_node_id, candidate_id, item_id, trigger_sequence, action_kind, idempotency_key, item_json, requested_by, request_note, request_status, created_at, updated_at, claimed_by, claimed_at, claimed_until, claim_token, completed_at, failed_at, canceled_at, stale_at, attempt_count, result_json, error_text
                 from remote_action_requests where origin_node_id = ?1 order by created_at asc, request_id asc",
            )
            .map_err(db_error)?;
        let mut rows = statement
            .query(params![request.origin_node_id.as_str()])
            .map_err(db_error)?;
        let mut requests = Vec::new();
        while let Some(row) = rows.next().map_err(db_error)? {
            let remote_request = Self::remote_action_request_from_row(row).map_err(db_error)?;
            if let Some(candidate_id) = request.candidate_id.as_ref()
                && &remote_request.candidate_id != candidate_id
            {
                continue;
            }
            if let Some(item_id) = request.item_id.as_ref()
                && &remote_request.item_id != item_id
            {
                continue;
            }
            if let Some(action_kind) = request.action_kind
                && remote_request.action_kind != action_kind
            {
                continue;
            }
            if let Some(status) = request.status
                && remote_request.status != status
            {
                continue;
            }
            if request.pending_only
                && remote_request.status != OperatorRemoteActionRequestStatus::Pending
            {
                continue;
            }
            if request.actionable_only
                && !Self::remote_action_item_is_actionable(&remote_request.item)
            {
                continue;
            }
            requests.push(remote_request);
            if let Some(limit) = request.limit
                && requests.len() >= limit
            {
                break;
            }
        }
        Ok(OperatorRemoteActionListResponse {
            origin_node_id: request.origin_node_id.clone(),
            requests,
        })
    }

    pub fn get_remote_action_request(
        &self,
        request: &OperatorRemoteActionGetRequest,
    ) -> TTResult<OperatorRemoteActionGetResponse> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection
            .prepare(
                "select request_id, origin_node_id, candidate_id, item_id, trigger_sequence, action_kind, idempotency_key, item_json, requested_by, request_note, request_status, created_at, updated_at, claimed_by, claimed_at, claimed_until, claim_token, completed_at, failed_at, canceled_at, stale_at, attempt_count, result_json, error_text
                 from remote_action_requests where request_id = ?1",
            )
            .map_err(db_error)?;
        let request_row = statement
            .query_row(params![request.request_id.as_str()], |row| {
                Self::remote_action_request_from_row(row)
            })
            .optional()
            .map_err(db_error)?;
        let origin_node_id = request.origin_node_id.clone();
        let request = request_row.filter(|row| row.origin_node_id == origin_node_id);
        Ok(OperatorRemoteActionGetResponse {
            origin_node_id,
            request,
        })
    }

    pub fn claim_remote_action_requests(
        &self,
        request: &OperatorRemoteActionClaimRequest,
    ) -> TTResult<OperatorRemoteActionClaimResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let now = Utc::now();
        let lease_until =
            now + chrono::Duration::milliseconds(request.lease_ms.unwrap_or(30_000).max(1) as i64);
        let limit = request.limit.unwrap_or(1).max(1);
        let mut requests = Vec::new();
        {
            let mut statement = transaction
                .prepare(
                    "select request_id, origin_node_id, candidate_id, item_id, trigger_sequence, action_kind, idempotency_key, item_json, requested_by, request_note, request_status, created_at, updated_at, claimed_by, claimed_at, claimed_until, claim_token, completed_at, failed_at, canceled_at, stale_at, attempt_count, result_json, error_text
                 from remote_action_requests where origin_node_id = ?1 order by created_at asc, request_id asc",
                )
                .map_err(db_error)?;
            let mut rows = statement
                .query(params![request.origin_node_id.as_str()])
                .map_err(db_error)?;
            while let Some(row) = rows.next().map_err(db_error)? {
                if requests.len() >= limit {
                    break;
                }
                let candidate_request =
                    Self::remote_action_request_from_row(row).map_err(db_error)?;
                let claimable = match candidate_request.status {
                    OperatorRemoteActionRequestStatus::Pending => true,
                    OperatorRemoteActionRequestStatus::Claimed => candidate_request
                        .claimed_until
                        .is_none_or(|until| until <= now),
                    _ => false,
                };
                if !claimable {
                    continue;
                }
                let candidate = Self::load_notification_candidate_tx(
                    &transaction,
                    request.origin_node_id.as_str(),
                    candidate_request.candidate_id.as_str(),
                )?;
                if candidate.as_ref().is_none_or(|candidate| {
                    !Self::remote_action_item_is_actionable(&candidate.item)
                }) {
                    Self::update_remote_action_request_status_tx(
                        &transaction,
                        candidate_request.request_id.as_str(),
                        OperatorRemoteActionRequestStatus::Stale,
                        now,
                        0,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        Some(now),
                        None,
                        Some("mirrored inbox item is no longer actionable".to_string()),
                    )?;
                    continue;
                }
                let claim_token = Uuid::new_v4().to_string();
                Self::update_remote_action_request_status_tx(
                    &transaction,
                    candidate_request.request_id.as_str(),
                    OperatorRemoteActionRequestStatus::Claimed,
                    now,
                    1,
                    Some(request.worker_id.as_str()),
                    Some(now),
                    Some(lease_until),
                    Some(claim_token.as_str()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                let request_row = Self::load_remote_action_request_tx(
                    &transaction,
                    candidate_request.request_id.as_str(),
                )?
                .ok_or_else(|| {
                    TTError::Store("remote action request disappeared during claim".to_string())
                })?;
                requests.push(OperatorRemoteActionClaimedRequest {
                    request: request_row,
                    claim_token,
                    claimed_until: lease_until,
                });
            }
        }
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(OperatorRemoteActionClaimResponse {
            origin_node_id: request.origin_node_id.clone(),
            requests,
        })
    }

    pub fn complete_remote_action_request(
        &self,
        request: &OperatorRemoteActionCompleteRequest,
    ) -> TTResult<OperatorRemoteActionCompleteResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let existing =
            Self::load_remote_action_request_tx(&transaction, request.request_id.as_str())?
                .ok_or_else(|| TTError::Store("remote action request not found".to_string()))?;
        if existing.origin_node_id != request.origin_node_id {
            return Err(TTError::Store(
                "remote action request origin mismatch".to_string(),
            ));
        }
        if existing.claim_token.as_deref() != Some(request.claim_token.as_str()) {
            return Err(TTError::Store(
                "remote action request claim token mismatch".to_string(),
            ));
        }
        if matches!(
            existing.status,
            OperatorRemoteActionRequestStatus::Completed
                | OperatorRemoteActionRequestStatus::Failed
                | OperatorRemoteActionRequestStatus::Canceled
                | OperatorRemoteActionRequestStatus::Stale
        ) {
            transaction.commit().map_err(db_error)?;
            return Ok(OperatorRemoteActionCompleteResponse {
                origin_node_id: request.origin_node_id.clone(),
                request: existing,
            });
        }
        let now = Utc::now();
        Self::update_remote_action_request_status_tx(
            &transaction,
            request.request_id.as_str(),
            OperatorRemoteActionRequestStatus::Completed,
            now,
            0,
            None,
            None,
            None,
            None,
            Some(now),
            None,
            None,
            None,
            Some(request.result.clone()),
            None,
        )?;
        let updated =
            Self::load_remote_action_request_tx(&transaction, request.request_id.as_str())?
                .ok_or_else(|| {
                    TTError::Store(
                        "remote action request disappeared during completion".to_string(),
                    )
                })?;
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(OperatorRemoteActionCompleteResponse {
            origin_node_id: request.origin_node_id.clone(),
            request: updated,
        })
    }

    pub fn fail_remote_action_request(
        &self,
        request: &OperatorRemoteActionFailRequest,
    ) -> TTResult<OperatorRemoteActionFailResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let existing =
            Self::load_remote_action_request_tx(&transaction, request.request_id.as_str())?
                .ok_or_else(|| TTError::Store("remote action request not found".to_string()))?;
        if existing.origin_node_id != request.origin_node_id {
            return Err(TTError::Store(
                "remote action request origin mismatch".to_string(),
            ));
        }
        if existing.claim_token.as_deref() != Some(request.claim_token.as_str()) {
            return Err(TTError::Store(
                "remote action request claim token mismatch".to_string(),
            ));
        }
        if matches!(
            existing.status,
            OperatorRemoteActionRequestStatus::Completed
                | OperatorRemoteActionRequestStatus::Failed
                | OperatorRemoteActionRequestStatus::Canceled
                | OperatorRemoteActionRequestStatus::Stale
        ) {
            transaction.commit().map_err(db_error)?;
            return Ok(OperatorRemoteActionFailResponse {
                origin_node_id: request.origin_node_id.clone(),
                request: existing,
            });
        }
        let now = Utc::now();
        Self::update_remote_action_request_status_tx(
            &transaction,
            request.request_id.as_str(),
            OperatorRemoteActionRequestStatus::Failed,
            now,
            0,
            None,
            None,
            None,
            None,
            None,
            Some(now),
            None,
            None,
            None,
            Some(request.error.clone()),
        )?;
        let updated =
            Self::load_remote_action_request_tx(&transaction, request.request_id.as_str())?
                .ok_or_else(|| {
                    TTError::Store("remote action request disappeared during failure".to_string())
                })?;
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(OperatorRemoteActionFailResponse {
            origin_node_id: request.origin_node_id.clone(),
            request: updated,
        })
    }

    pub fn upsert_notification_recipient(
        &self,
        request: &NotificationRecipientUpsertRequest,
    ) -> TTResult<NotificationRecipientUpsertResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let now = Utc::now();
        transaction.execute(
            "insert into notification_recipients(recipient_id, display_name, enabled, created_at, updated_at)
             values(?1, ?2, ?3, ?4, ?5)
             on conflict(recipient_id) do update set
               display_name = excluded.display_name,
               enabled = excluded.enabled,
               updated_at = excluded.updated_at",
            params![
                request.recipient_id.as_str(),
                request.display_name.as_str(),
                request.enabled as i64,
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        ).map_err(db_error)?;
        let recipient = Self::load_recipient_tx(&transaction, request.recipient_id.as_str())?
            .expect("recipient just upserted");
        if recipient.enabled {
            self.enqueue_delivery_jobs_for_recipient_tx(
                &transaction,
                recipient.recipient_id.as_str(),
                now,
            )?;
        } else {
            self.disable_delivery_jobs_for_recipient_tx(
                &transaction,
                recipient.recipient_id.as_str(),
                now,
            )?;
        }
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(NotificationRecipientUpsertResponse { recipient })
    }

    pub fn list_notification_recipients(
        &self,
        request: &NotificationRecipientListRequest,
    ) -> TTResult<NotificationRecipientListResponse> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection
            .prepare(
                "select recipient_id, display_name, enabled, created_at, updated_at
                 from notification_recipients
                 order by updated_at desc, recipient_id asc",
            )
            .map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        let mut recipients = Vec::new();
        while let Some(row) = rows.next().map_err(db_error)? {
            let recipient = Self::recipient_from_row(row).map_err(db_error)?;
            if !request.include_disabled && !recipient.enabled {
                continue;
            }
            recipients.push(recipient);
        }
        Ok(NotificationRecipientListResponse { recipients })
    }

    pub fn upsert_notification_subscription(
        &self,
        request: &NotificationSubscriptionUpsertRequest,
    ) -> TTResult<NotificationSubscriptionUpsertResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let now = Utc::now();
        transaction.execute(
            "insert into notification_subscriptions(subscription_id, recipient_id, transport_kind, endpoint_json, enabled, created_at, updated_at)
             values(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             on conflict(subscription_id) do update set
               recipient_id = excluded.recipient_id,
               transport_kind = excluded.transport_kind,
               endpoint_json = excluded.endpoint_json,
               enabled = excluded.enabled,
               updated_at = excluded.updated_at",
            params![
                request.subscription_id.as_str(),
                request.recipient_id.as_str(),
                Self::transport_kind_to_str(request.transport_kind),
                request.endpoint.to_string(),
                request.enabled as i64,
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        ).map_err(db_error)?;
        let subscription =
            Self::load_subscription_tx(&transaction, request.subscription_id.as_str())?
                .expect("subscription just upserted");
        if subscription.enabled {
            self.enqueue_delivery_jobs_for_subscription_tx(
                &transaction,
                subscription.subscription_id.as_str(),
                now,
            )?;
        } else {
            self.disable_delivery_jobs_for_subscription_tx(
                &transaction,
                subscription.subscription_id.as_str(),
                now,
            )?;
        }
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(NotificationSubscriptionUpsertResponse { subscription })
    }

    pub fn list_notification_subscriptions(
        &self,
        request: &NotificationSubscriptionListRequest,
    ) -> TTResult<NotificationSubscriptionListResponse> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection
            .prepare(
                "select subscription_id, recipient_id, transport_kind, endpoint_json, enabled, created_at, updated_at
                 from notification_subscriptions
                 order by updated_at desc, subscription_id asc",
            )
            .map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        let mut subscriptions = Vec::new();
        while let Some(row) = rows.next().map_err(db_error)? {
            let subscription = Self::subscription_from_row(row).map_err(db_error)?;
            if let Some(recipient_id) = request.recipient_id.as_ref() {
                if subscription.recipient_id != *recipient_id {
                    continue;
                }
            }
            if request.enabled_only && !subscription.enabled {
                continue;
            }
            subscriptions.push(subscription);
        }
        Ok(NotificationSubscriptionListResponse { subscriptions })
    }

    pub fn set_notification_subscription_enabled(
        &self,
        request: &NotificationSubscriptionSetEnabledRequest,
    ) -> TTResult<NotificationSubscriptionSetEnabledResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let now = Utc::now();
        transaction.execute(
            "update notification_subscriptions set enabled = ?2, updated_at = ?3 where subscription_id = ?1",
            params![
                request.subscription_id.as_str(),
                request.enabled as i64,
                now.to_rfc3339(),
            ],
        ).map_err(db_error)?;
        let subscription =
            Self::load_subscription_tx(&transaction, request.subscription_id.as_str())?
                .ok_or_else(|| TTError::Store("notification subscription not found".to_string()))?;
        if subscription.enabled {
            self.enqueue_delivery_jobs_for_subscription_tx(
                &transaction,
                subscription.subscription_id.as_str(),
                now,
            )?;
        } else {
            self.disable_delivery_jobs_for_subscription_tx(
                &transaction,
                subscription.subscription_id.as_str(),
                now,
            )?;
        }
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(NotificationSubscriptionSetEnabledResponse { subscription })
    }

    pub fn list_notification_delivery_jobs(
        &self,
        request: &NotificationDeliveryJobListRequest,
    ) -> TTResult<NotificationDeliveryJobListResponse> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection
            .prepare(
            "select job_id, origin_node_id, candidate_id, trigger_sequence, recipient_id, subscription_id, transport_kind, status, attempt_count, created_at, updated_at, dispatched_at, delivered_at, failed_at, suppressed_at, skipped_at, obsolete_at, receipt_json, error_text
                 from notification_delivery_jobs
                 order by updated_at desc, job_id asc",
            )
            .map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        let mut jobs = Vec::new();
        while let Some(row) = rows.next().map_err(db_error)? {
            let job = Self::job_from_row(row).map_err(db_error)?;
            if let Some(origin) = request.origin_node_id.as_ref() {
                if job.origin_node_id != *origin {
                    continue;
                }
            }
            if let Some(candidate_id) = request.candidate_id.as_ref() {
                if job.candidate_id != *candidate_id {
                    continue;
                }
            }
            if let Some(subscription_id) = request.subscription_id.as_ref() {
                if job.subscription_id != *subscription_id {
                    continue;
                }
            }
            if let Some(status) = request.status {
                if job.status != status {
                    continue;
                }
            }
            jobs.push(job);
        }
        if let Some(limit) = request.limit {
            jobs.truncate(limit);
        }
        Ok(NotificationDeliveryJobListResponse { jobs })
    }

    pub fn get_notification_delivery_job(
        &self,
        request: &NotificationDeliveryJobGetRequest,
    ) -> TTResult<Option<NotificationDeliveryJob>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let mut statement = connection
            .prepare(
            "select job_id, origin_node_id, candidate_id, trigger_sequence, recipient_id, subscription_id, transport_kind, status, attempt_count, created_at, updated_at, dispatched_at, delivered_at, failed_at, suppressed_at, skipped_at, obsolete_at, receipt_json, error_text
                 from notification_delivery_jobs where job_id = ?1",
            )
            .map_err(db_error)?;
        let job = statement
            .query_row(params![request.job_id.as_str()], |row| {
                Self::job_from_row(row)
            })
            .optional()
            .map_err(db_error)?;
        Ok(job)
    }

    pub fn dispatch_pending_notification_delivery_jobs<
        T: NotificationDeliveryTransport + ?Sized,
    >(
        &self,
        transport: &T,
        limit: Option<usize>,
    ) -> TTResult<NotificationDeliveryRunPendingResponse> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TTError::Store("mirror store connection lock poisoned".to_string()))?;
        let transaction = connection.transaction().map_err(db_error)?;
        let transport_kind = Self::transport_kind_to_str(transport.kind());
        let mut statement = transaction
            .prepare(
            "select job_id, origin_node_id, candidate_id, trigger_sequence, recipient_id, subscription_id, transport_kind, status, attempt_count, created_at, updated_at, dispatched_at, delivered_at, failed_at, suppressed_at, skipped_at, obsolete_at, receipt_json, error_text
                 from notification_delivery_jobs where status = ?1 and transport_kind = ?2 order by created_at asc, job_id asc",
            )
            .map_err(db_error)?;
        let mut rows = statement
            .query(params![
                Self::delivery_job_status_to_str(NotificationDeliveryJobStatus::Pending),
                transport_kind,
            ])
            .map_err(db_error)?;
        let mut jobs = Vec::new();
        while let Some(row) = rows.next().map_err(db_error)? {
            let job = Self::job_from_row(row).map_err(db_error)?;
            let candidate = Self::load_notification_candidate_tx(
                &transaction,
                job.origin_node_id.as_str(),
                job.candidate_id.as_str(),
            )?;
            let subscription =
                Self::load_subscription_tx(&transaction, job.subscription_id.as_str())?;
            let recipient = subscription.as_ref().and_then(|subscription| {
                Self::load_recipient_tx(&transaction, subscription.recipient_id.as_str())
                    .ok()
                    .flatten()
            });
            let updated = self.dispatch_job_tx(
                &transaction,
                transport,
                job,
                candidate,
                subscription,
                recipient,
                Utc::now(),
            )?;
            jobs.push(updated);
            if let Some(limit) = limit {
                if jobs.len() >= limit {
                    break;
                }
            }
        }
        drop(rows);
        drop(statement);
        transaction.commit().map_err(db_error)?;
        self.notify_checkpoint_changed();
        Ok(NotificationDeliveryRunPendingResponse { jobs })
    }

    fn load_checkpoint_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
    ) -> TTResult<OperatorInboxCheckpoint> {
        let mut statement = transaction.prepare(
            "select current_sequence, updated_at from mirrored_inbox_checkpoint where origin_node_id = ?1",
        ).map_err(db_error)?;
        let checkpoint = statement
            .query_row(params![origin_node_id], |row| {
                Ok(OperatorInboxCheckpoint {
                    current_sequence: row.get::<_, i64>(0)? as u64,
                    updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(1)?)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                })
            })
            .optional()
            .map_err(db_error)?;
        Ok(checkpoint.unwrap_or_default())
    }

    fn load_mirrored_item_tx(
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
        item_id: &str,
    ) -> TTResult<Option<OperatorInboxItem>> {
        let mut statement = transaction
            .prepare("select item_json from mirrored_inbox_items where origin_node_id = ?1 and item_id = ?2")
            .map_err(db_error)?;
        let item = statement
            .query_row(params![origin_node_id, item_id], |row| {
                let item_json = row.get::<_, String>(0)?;
                serde_json::from_str::<OperatorInboxItem>(&item_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .optional()
            .map_err(db_error)?;
        Ok(item)
    }

    fn candidate_from_row(
        row: &rusqlite::Row<'_>,
    ) -> Result<OperatorNotificationCandidate, rusqlite::Error> {
        let item_json = row.get::<_, String>(5)?;
        let item = serde_json::from_str::<OperatorInboxItem>(&item_json).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let status = match row.get::<_, String>(4)?.as_str() {
            "pending" => OperatorNotificationCandidateStatus::Pending,
            "acknowledged" => OperatorNotificationCandidateStatus::Acknowledged,
            "suppressed" => OperatorNotificationCandidateStatus::Suppressed,
            "obsolete" => OperatorNotificationCandidateStatus::Obsolete,
            other => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unknown notification candidate status `{other}`"),
                    )),
                ));
            }
        };
        let parse_ts = |index: usize| -> Result<DateTime<Utc>, rusqlite::Error> {
            DateTime::parse_from_rfc3339(&row.get::<_, String>(index)?)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
        };
        let parse_optional_ts = |index: usize| -> Result<Option<DateTime<Utc>>, rusqlite::Error> {
            row.get::<_, Option<String>>(index)?
                .map(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .map(|value| value.with_timezone(&Utc))
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                index,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })
                })
                .transpose()
        };
        Ok(OperatorNotificationCandidate {
            candidate_id: row.get::<_, String>(0)?,
            origin_node_id: row.get::<_, String>(1)?,
            item_id: row.get::<_, String>(2)?,
            trigger_sequence: row.get::<_, i64>(3)? as u64,
            status,
            item,
            created_at: parse_ts(6)?,
            updated_at: parse_ts(7)?,
            acknowledged_at: parse_optional_ts(8)?,
            suppressed_at: parse_optional_ts(9)?,
            resolved_at: parse_optional_ts(10)?,
        })
    }

    fn inbox_item_is_actionable(item: &OperatorInboxItem) -> bool {
        item.status == tt_core::ipc::OperatorInboxItemStatus::Open
            && !item.available_actions.is_empty()
    }

    fn candidate_is_actionable(candidate: &OperatorNotificationCandidate) -> bool {
        candidate.status != OperatorNotificationCandidateStatus::Obsolete
            && Self::inbox_item_is_actionable(&candidate.item)
    }

    fn notification_candidate_id(
        origin_node_id: &str,
        item_id: &str,
        trigger_sequence: u64,
    ) -> String {
        format!("{origin_node_id}::{item_id}::{trigger_sequence}")
    }

    fn candidate_status_to_str(status: OperatorNotificationCandidateStatus) -> &'static str {
        match status {
            OperatorNotificationCandidateStatus::Pending => "pending",
            OperatorNotificationCandidateStatus::Acknowledged => "acknowledged",
            OperatorNotificationCandidateStatus::Suppressed => "suppressed",
            OperatorNotificationCandidateStatus::Obsolete => "obsolete",
        }
    }

    fn load_notification_window_tx(
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
        item_id: &str,
    ) -> TTResult<Option<(String, u64)>> {
        let mut statement = transaction
            .prepare("select candidate_id, opened_sequence from mirrored_notification_windows where origin_node_id = ?1 and item_id = ?2")
            .map_err(db_error)?;
        let window = statement
            .query_row(params![origin_node_id, item_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
            })
            .optional()
            .map_err(db_error)?;
        Ok(window)
    }

    fn load_notification_candidate_tx(
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
        candidate_id: &str,
    ) -> TTResult<Option<OperatorNotificationCandidate>> {
        let mut statement = transaction
            .prepare("select candidate_id, origin_node_id, item_id, trigger_sequence, candidate_status, item_json, created_at, updated_at, acknowledged_at, suppressed_at, resolved_at, obsolete_at from mirrored_notification_candidates where origin_node_id = ?1 and candidate_id = ?2")
            .map_err(db_error)?;
        let candidate = statement
            .query_row(params![origin_node_id, candidate_id], |row| {
                Self::candidate_from_row(row)
            })
            .optional()
            .map_err(db_error)?;
        Ok(candidate)
    }

    fn apply_notification_transition_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
        previous_item: Option<&OperatorInboxItem>,
        current_item: Option<&OperatorInboxItem>,
        sequence: u64,
        changed_at: DateTime<Utc>,
    ) -> TTResult<()> {
        let previous_actionable = previous_item
            .map(Self::inbox_item_is_actionable)
            .unwrap_or(false);
        let current_actionable = current_item
            .map(Self::inbox_item_is_actionable)
            .unwrap_or(false);
        let item_id = current_item
            .map(|item| item.id.as_str())
            .or_else(|| previous_item.map(|item| item.id.as_str()))
            .ok_or_else(|| {
                TTError::Store("notification transition missing item identity".to_string())
            })?;
        let existing_window =
            Self::load_notification_window_tx(transaction, origin_node_id, item_id)?;

        match (previous_actionable, current_actionable) {
            (false, true) => {
                let item =
                    current_item.expect("current item should exist when becoming actionable");
                if existing_window.is_some() {
                    self.update_notification_candidate_snapshot_tx(
                        transaction,
                        origin_node_id,
                        item.id.as_str(),
                        item,
                        sequence,
                        changed_at,
                    )?;
                } else {
                    self.create_notification_candidate_tx(
                        transaction,
                        origin_node_id,
                        item,
                        sequence,
                        changed_at,
                    )?;
                }
                let candidate_id = existing_window
                    .as_ref()
                    .map(|(candidate_id, _)| candidate_id.clone())
                    .unwrap_or_else(|| {
                        Self::notification_candidate_id(origin_node_id, item.id.as_str(), sequence)
                    });
                if let Some(candidate) = Self::load_notification_candidate_tx(
                    transaction,
                    origin_node_id,
                    candidate_id.as_str(),
                )? {
                    self.enqueue_delivery_jobs_for_candidate_tx(
                        transaction,
                        &candidate,
                        changed_at,
                    )?;
                }
            }
            (true, true) => {
                let item =
                    current_item.expect("current item should exist when remaining actionable");
                if existing_window.is_none() {
                    self.create_notification_candidate_tx(
                        transaction,
                        origin_node_id,
                        item,
                        sequence,
                        changed_at,
                    )?;
                } else {
                    self.update_notification_candidate_snapshot_tx(
                        transaction,
                        origin_node_id,
                        item.id.as_str(),
                        item,
                        sequence,
                        changed_at,
                    )?;
                }
                if let Some((candidate_id, _)) =
                    Self::load_notification_window_tx(transaction, origin_node_id, item_id)?
                {
                    if let Some(candidate) = Self::load_notification_candidate_tx(
                        transaction,
                        origin_node_id,
                        candidate_id.as_str(),
                    )? {
                        self.enqueue_delivery_jobs_for_candidate_tx(
                            transaction,
                            &candidate,
                            changed_at,
                        )?;
                    }
                }
            }
            (true, false) => {
                self.close_notification_candidate_tx(
                    transaction,
                    origin_node_id,
                    item_id,
                    current_item.or(previous_item),
                    changed_at,
                )?;
            }
            (false, false) => {}
        }
        Ok(())
    }

    fn create_notification_candidate_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
        item: &OperatorInboxItem,
        sequence: u64,
        changed_at: DateTime<Utc>,
    ) -> TTResult<()> {
        let candidate_id =
            Self::notification_candidate_id(origin_node_id, item.id.as_str(), sequence);
        let item_json = serde_json::to_string(item)?;
        transaction.execute(
            "insert into mirrored_notification_candidates(candidate_id, origin_node_id, item_id, trigger_sequence, candidate_status, item_json, created_at, updated_at, acknowledged_at, suppressed_at, resolved_at, obsolete_at)
             values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, null, null, null, null)
             on conflict(candidate_id) do update set item_json = excluded.item_json, updated_at = excluded.updated_at",
            params![
                candidate_id,
                origin_node_id,
                item.id.as_str(),
                sequence as i64,
                Self::candidate_status_to_str(OperatorNotificationCandidateStatus::Pending),
                item_json,
                changed_at.to_rfc3339(),
                changed_at.to_rfc3339(),
            ],
        ).map_err(db_error)?;
        transaction.execute(
            "insert into mirrored_notification_windows(origin_node_id, item_id, candidate_id, opened_sequence, updated_sequence, updated_at)
             values(?1, ?2, ?3, ?4, ?5, ?6)
             on conflict(origin_node_id, item_id) do update set
               candidate_id = excluded.candidate_id,
               opened_sequence = excluded.opened_sequence,
               updated_sequence = excluded.updated_sequence,
               updated_at = excluded.updated_at",
            params![
                origin_node_id,
                item.id.as_str(),
                candidate_id,
                sequence as i64,
                sequence as i64,
                changed_at.to_rfc3339(),
            ],
        ).map_err(db_error)?;
        Ok(())
    }

    fn update_notification_candidate_snapshot_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
        item_id: &str,
        item: &OperatorInboxItem,
        sequence: u64,
        changed_at: DateTime<Utc>,
    ) -> TTResult<()> {
        let Some((candidate_id, _)) =
            Self::load_notification_window_tx(transaction, origin_node_id, item_id)?
        else {
            return self.create_notification_candidate_tx(
                transaction,
                origin_node_id,
                item,
                sequence,
                changed_at,
            );
        };
        let item_json = serde_json::to_string(item)?;
        transaction.execute(
            "update mirrored_notification_candidates set item_json = ?3, updated_at = ?4 where candidate_id = ?1 and origin_node_id = ?2",
            params![candidate_id, origin_node_id, item_json, changed_at.to_rfc3339()],
        ).map_err(db_error)?;
        transaction.execute(
            "update mirrored_notification_windows set updated_sequence = ?3, updated_at = ?4 where origin_node_id = ?1 and item_id = ?2",
            params![origin_node_id, item_id, sequence as i64, changed_at.to_rfc3339()],
        ).map_err(db_error)?;
        Ok(())
    }

    fn close_notification_candidate_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
        item_id: &str,
        item: Option<&OperatorInboxItem>,
        changed_at: DateTime<Utc>,
    ) -> TTResult<()> {
        let Some((candidate_id, _)) =
            Self::load_notification_window_tx(transaction, origin_node_id, item_id)?
        else {
            return Ok(());
        };
        if let Some(item_json) = item.map(serde_json::to_string).transpose()? {
            transaction
                .execute(
                    "update mirrored_notification_candidates
                 set item_json = ?3,
                     candidate_status = ?4,
                     updated_at = ?5,
                     resolved_at = coalesce(resolved_at, ?5),
                     obsolete_at = coalesce(obsolete_at, ?5)
                 where candidate_id = ?1 and origin_node_id = ?2",
                    params![
                        candidate_id,
                        origin_node_id,
                        item_json,
                        Self::candidate_status_to_str(
                            OperatorNotificationCandidateStatus::Obsolete
                        ),
                        changed_at.to_rfc3339(),
                    ],
                )
                .map_err(db_error)?;
        } else {
            transaction
                .execute(
                    "update mirrored_notification_candidates
                 set candidate_status = ?3,
                     updated_at = ?4,
                     resolved_at = coalesce(resolved_at, ?4),
                     obsolete_at = coalesce(obsolete_at, ?4)
                 where candidate_id = ?1 and origin_node_id = ?2",
                    params![
                        candidate_id,
                        origin_node_id,
                        Self::candidate_status_to_str(
                            OperatorNotificationCandidateStatus::Obsolete
                        ),
                        changed_at.to_rfc3339(),
                    ],
                )
                .map_err(db_error)?;
        }
        self.mark_delivery_jobs_for_candidate_status_tx(
            transaction,
            candidate_id.as_str(),
            NotificationDeliveryJobStatus::Obsolete,
            changed_at,
        )?;
        self.mark_remote_action_requests_for_candidate_status_tx(
            transaction,
            candidate_id.as_str(),
            OperatorRemoteActionRequestStatus::Stale,
            changed_at,
        )?;
        transaction.execute(
            "delete from mirrored_notification_windows where origin_node_id = ?1 and item_id = ?2",
            params![origin_node_id, item_id],
        ).map_err(db_error)?;
        Ok(())
    }

    fn write_notification_candidate_status_tx(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        origin_node_id: &str,
        candidate_id: &str,
        status: OperatorNotificationCandidateStatus,
        updated_at: DateTime<Utc>,
        acknowledged_at: Option<DateTime<Utc>>,
        suppressed_at: Option<DateTime<Utc>>,
        resolved_at: Option<DateTime<Utc>>,
        obsolete_at: Option<DateTime<Utc>>,
    ) -> TTResult<OperatorNotificationCandidate> {
        let existing =
            Self::load_notification_candidate_tx(transaction, origin_node_id, candidate_id)?
                .ok_or_else(|| TTError::Store("notification candidate not found".to_string()))?;
        transaction
            .execute(
                "update mirrored_notification_candidates
             set candidate_status = ?3,
                 updated_at = ?4,
                 acknowledged_at = coalesce(?5, acknowledged_at),
                 suppressed_at = coalesce(?6, suppressed_at),
                 resolved_at = coalesce(?7, resolved_at),
                 obsolete_at = coalesce(?8, obsolete_at)
             where origin_node_id = ?1 and candidate_id = ?2",
                params![
                    origin_node_id,
                    candidate_id,
                    Self::candidate_status_to_str(status),
                    updated_at.to_rfc3339(),
                    acknowledged_at.map(|value| value.to_rfc3339()),
                    suppressed_at.map(|value| value.to_rfc3339()),
                    resolved_at.map(|value| value.to_rfc3339()),
                    obsolete_at.map(|value| value.to_rfc3339()),
                ],
            )
            .map_err(db_error)?;
        Self::load_notification_candidate_tx(transaction, origin_node_id, candidate_id)?
            .or(Some(existing))
            .ok_or_else(|| {
                TTError::Store("notification candidate disappeared during update".to_string())
            })
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;

    use super::*;
    use crate::delivery::{MockNotificationDeliveryTransport, NotificationDeliveryOutcome};
    use tt_core::ipc::{
        NotificationDeliveryJobListRequest, NotificationDeliveryJobStatus,
        NotificationRecipientUpsertRequest, NotificationSubscriptionUpsertRequest,
        NotificationTransportKind, OperatorInboxActionKind, OperatorInboxChange,
        OperatorInboxChangeKind, OperatorInboxItem, OperatorInboxItemStatus,
        OperatorInboxSourceKind, OperatorNotificationAckRequest,
        OperatorReadModelCheckpointQueryRequest, OperatorReadModelWaitForCheckpointRequest,
        OperatorRemoteActionClaimRequest, OperatorRemoteActionCompleteRequest,
        OperatorRemoteActionCreateRequest, OperatorRemoteActionFailRequest,
        OperatorRemoteActionGetRequest, OperatorRemoteActionListRequest,
        OperatorRemoteActionRequestStatus,
    };

    fn ts(offset: i64) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0)
            .single()
            .expect("base timestamp")
            + chrono::Duration::seconds(offset)
    }

    fn item(
        id: &str,
        sequence: u64,
        title: &str,
        updated_at: chrono::DateTime<Utc>,
    ) -> OperatorInboxItem {
        OperatorInboxItem {
            id: id.to_string(),
            sequence,
            source_kind: OperatorInboxSourceKind::SupervisorProposal,
            actionable_object_id: id.to_string(),
            workstream_id: Some("workstream-1".to_string()),
            work_unit_id: Some("work-unit-1".to_string()),
            title: title.to_string(),
            summary: format!("summary {title}"),
            status: OperatorInboxItemStatus::Open,
            available_actions: vec![OperatorInboxActionKind::Approve],
            created_at: updated_at,
            updated_at,
            resolved_at: None,
            rationale: None,
            provenance: Some("source=proposal".to_string()),
        }
    }

    fn change(
        sequence: u64,
        kind: OperatorInboxChangeKind,
        item: OperatorInboxItem,
    ) -> OperatorInboxChange {
        OperatorInboxChange {
            sequence,
            kind,
            item,
            changed_at: ts(sequence as i64),
        }
    }

    fn open_notification_request(origin_node_id: &str) -> OperatorNotificationListRequest {
        OperatorNotificationListRequest {
            origin_node_id: origin_node_id.to_string(),
            status: None,
            pending_only: false,
            actionable_only: false,
            limit: None,
        }
    }

    fn recipient_request(
        recipient_id: &str,
        display_name: &str,
        enabled: bool,
    ) -> NotificationRecipientUpsertRequest {
        NotificationRecipientUpsertRequest {
            recipient_id: recipient_id.to_string(),
            display_name: display_name.to_string(),
            enabled,
        }
    }

    fn subscription_request(
        subscription_id: &str,
        recipient_id: &str,
        transport_kind: NotificationTransportKind,
        enabled: bool,
    ) -> NotificationSubscriptionUpsertRequest {
        NotificationSubscriptionUpsertRequest {
            subscription_id: subscription_id.to_string(),
            recipient_id: recipient_id.to_string(),
            transport_kind,
            endpoint: json!({
                "endpoint": format!("https://example.invalid/{subscription_id}"),
            }),
            enabled,
        }
    }

    fn delivery_jobs_request(origin_node_id: &str) -> NotificationDeliveryJobListRequest {
        NotificationDeliveryJobListRequest {
            origin_node_id: Some(origin_node_id.to_string()),
            candidate_id: None,
            subscription_id: None,
            status: None,
            limit: None,
        }
    }

    fn remote_action_create_request(
        origin_node_id: &str,
        item_id: &str,
        action_kind: OperatorInboxActionKind,
    ) -> OperatorRemoteActionCreateRequest {
        OperatorRemoteActionCreateRequest {
            origin_node_id: origin_node_id.to_string(),
            item_id: item_id.to_string(),
            action_kind,
            idempotency_key: None,
            requested_by: Some("remote-operator".to_string()),
            request_note: Some("please execute".to_string()),
        }
    }

    fn remote_action_create_request_with_idempotency_key(
        origin_node_id: &str,
        item_id: &str,
        action_kind: OperatorInboxActionKind,
        idempotency_key: Option<&str>,
    ) -> OperatorRemoteActionCreateRequest {
        OperatorRemoteActionCreateRequest {
            origin_node_id: origin_node_id.to_string(),
            item_id: item_id.to_string(),
            action_kind,
            idempotency_key: idempotency_key.map(|value| value.to_string()),
            requested_by: Some("remote-operator".to_string()),
            request_note: Some("please execute".to_string()),
        }
    }

    fn remote_action_list_request(origin_node_id: &str) -> OperatorRemoteActionListRequest {
        OperatorRemoteActionListRequest {
            origin_node_id: origin_node_id.to_string(),
            candidate_id: None,
            item_id: None,
            action_kind: None,
            status: None,
            pending_only: false,
            actionable_only: false,
            limit: None,
        }
    }

    fn remote_action_get_request(
        origin_node_id: &str,
        request_id: &str,
    ) -> OperatorRemoteActionGetRequest {
        OperatorRemoteActionGetRequest {
            origin_node_id: origin_node_id.to_string(),
            request_id: request_id.to_string(),
        }
    }

    fn remote_action_claim_request(
        origin_node_id: &str,
        worker_id: &str,
    ) -> OperatorRemoteActionClaimRequest {
        OperatorRemoteActionClaimRequest {
            origin_node_id: origin_node_id.to_string(),
            worker_id: worker_id.to_string(),
            limit: Some(8),
            lease_ms: Some(60_000),
        }
    }

    fn remote_action_claim_request_with_lease(
        origin_node_id: &str,
        worker_id: &str,
        lease_ms: u64,
    ) -> OperatorRemoteActionClaimRequest {
        OperatorRemoteActionClaimRequest {
            origin_node_id: origin_node_id.to_string(),
            worker_id: worker_id.to_string(),
            limit: Some(8),
            lease_ms: Some(lease_ms),
        }
    }

    fn remote_action_complete_request(
        origin_node_id: &str,
        request_id: &str,
        claim_token: &str,
    ) -> OperatorRemoteActionCompleteRequest {
        OperatorRemoteActionCompleteRequest {
            origin_node_id: origin_node_id.to_string(),
            request_id: request_id.to_string(),
            claim_token: claim_token.to_string(),
            result: json!({"status": "ok"}),
        }
    }

    fn remote_action_fail_request(
        origin_node_id: &str,
        request_id: &str,
        claim_token: &str,
        error: &str,
    ) -> OperatorRemoteActionFailRequest {
        OperatorRemoteActionFailRequest {
            origin_node_id: origin_node_id.to_string(),
            request_id: request_id.to_string(),
            claim_token: claim_token.to_string(),
            error: error.to_string(),
        }
    }

    fn resolved_item(
        id: &str,
        sequence: u64,
        title: &str,
        updated_at: chrono::DateTime<Utc>,
    ) -> OperatorInboxItem {
        OperatorInboxItem {
            id: id.to_string(),
            sequence,
            source_kind: OperatorInboxSourceKind::SupervisorProposal,
            actionable_object_id: id.to_string(),
            workstream_id: Some("workstream-1".to_string()),
            work_unit_id: Some("work-unit-1".to_string()),
            title: title.to_string(),
            summary: format!("summary {title}"),
            status: OperatorInboxItemStatus::Resolved,
            available_actions: Vec::new(),
            created_at: updated_at,
            updated_at,
            resolved_at: Some(updated_at),
            rationale: None,
            provenance: Some("source=proposal".to_string()),
        }
    }

    #[test]
    fn apply_batch_is_idempotent_and_overlap_safe() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";

        let first = change(
            1,
            OperatorInboxChangeKind::Upsert,
            item("proposal-1", 1, "one", ts(1)),
        );
        let second = change(
            2,
            OperatorInboxChangeKind::Upsert,
            item("proposal-2", 2, "two", ts(2)),
        );
        let third = change(
            3,
            OperatorInboxChangeKind::Removed,
            item("proposal-1", 1, "one", ts(3)),
        );

        let result = store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[first.clone(), second.clone()],
            )
            .expect("apply batch");
        assert_eq!(result.checkpoint.current_sequence, 2);
        assert_eq!(result.applied_changes, 2);

        let repeat = store
            .apply_batch(
                origin,
                result.checkpoint.clone(),
                &[first.clone(), second.clone()],
            )
            .expect("repeat batch");
        assert_eq!(repeat.checkpoint.current_sequence, 2);
        assert_eq!(repeat.applied_changes, 0);
        assert_eq!(store.list(origin, None).expect("list").items.len(), 2);

        let overlap = store
            .apply_batch(
                origin,
                result.checkpoint.clone(),
                &[second.clone(), third.clone()],
            )
            .expect("overlap batch");
        assert_eq!(overlap.checkpoint.current_sequence, 3);
        assert_eq!(overlap.applied_changes, 1);
        assert_eq!(store.list(origin, None).expect("list").items.len(), 1);
        assert_eq!(
            store.get(origin, "proposal-2").expect("get"),
            Some(item("proposal-2", 2, "two", ts(2)))
        );
    }

    #[test]
    fn checkpoint_persists_across_restart() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("server.db");
        let origin = "origin-a";
        let store = InboxMirrorStore::open(&path).expect("store");
        let change = change(
            1,
            OperatorInboxChangeKind::Upsert,
            item("proposal-1", 1, "one", ts(1)),
        );
        let result = store
            .apply_batch(origin, OperatorInboxCheckpoint::default(), &[change])
            .expect("apply");
        drop(store);

        let reopened = InboxMirrorStore::open(&path).expect("reopen");
        let checkpoint = reopened.checkpoint(origin).expect("checkpoint");
        assert_eq!(
            checkpoint.current_sequence,
            result.checkpoint.current_sequence
        );
        assert_eq!(reopened.list(origin, None).expect("list").items.len(), 1);
    }

    #[tokio::test]
    async fn notification_checkpoint_wait_resolves_after_candidate_change() {
        let dir = tempdir().expect("tempdir");
        let store = Arc::new(InboxMirrorStore::open(dir.path().join("server.db")).expect("store"));
        let origin = "origin-a";
        store
            .as_ref()
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let current = store
            .as_ref()
            .notification_checkpoint(&OperatorReadModelCheckpointQueryRequest {
                origin_node_id: origin.to_string(),
            })
            .expect("checkpoint");
        let wait_store = store.clone();
        let wait = tokio::spawn(async move {
            wait_store
                .wait_for_notification_checkpoint(&OperatorReadModelWaitForCheckpointRequest {
                    origin_node_id: origin.to_string(),
                    after_updated_at: current.checkpoint.updated_at,
                    timeout_ms: Some(10_000),
                })
                .await
        });

        let candidate_id = format!("{origin}::proposal-1::1");
        let updated = store
            .as_ref()
            .acknowledge_notification_candidate(&OperatorNotificationAckRequest {
                origin_node_id: origin.to_string(),
                candidate_id,
            })
            .expect("ack");
        let waited = wait.await.expect("wait task").expect("wait result");
        assert!(!waited.timed_out);
        assert!(waited.checkpoint.updated_at.is_some());
        assert!(updated.candidate.updated_at <= waited.checkpoint.updated_at.unwrap());
    }

    #[tokio::test]
    async fn delivery_checkpoint_wait_resolves_after_delivery_change() {
        let dir = tempdir().expect("tempdir");
        let store = Arc::new(InboxMirrorStore::open(dir.path().join("server.db")).expect("store"));
        let origin = "origin-a";
        store
            .as_ref()
            .upsert_notification_recipient(&NotificationRecipientUpsertRequest {
                recipient_id: "recipient-1".to_string(),
                display_name: "Recipient 1".to_string(),
                enabled: true,
            })
            .expect("recipient");
        store
            .as_ref()
            .upsert_notification_subscription(&NotificationSubscriptionUpsertRequest {
                subscription_id: "subscription-1".to_string(),
                recipient_id: "recipient-1".to_string(),
                transport_kind: NotificationTransportKind::Mock,
                endpoint: serde_json::json!({"endpoint": "https://example.invalid/subscription-1"}),
                enabled: true,
            })
            .expect("subscription");
        store
            .as_ref()
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let current = store
            .as_ref()
            .delivery_checkpoint(&OperatorReadModelCheckpointQueryRequest {
                origin_node_id: origin.to_string(),
            })
            .expect("checkpoint");
        let wait_store = store.clone();
        let wait = tokio::spawn(async move {
            wait_store
                .wait_for_delivery_checkpoint(&OperatorReadModelWaitForCheckpointRequest {
                    origin_node_id: origin.to_string(),
                    after_updated_at: current.checkpoint.updated_at,
                    timeout_ms: Some(10_000),
                })
                .await
        });

        let result = store
            .as_ref()
            .dispatch_pending_notification_delivery_jobs(
                &MockNotificationDeliveryTransport::default(),
                Some(1),
            )
            .expect("dispatch");
        assert_eq!(result.jobs.len(), 1);
        let waited = wait.await.expect("wait task").expect("wait result");
        assert!(!waited.timed_out);
        assert!(waited.checkpoint.updated_at.is_some());
    }

    #[test]
    fn newly_actionable_item_creates_one_pending_notification_candidate() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        let action = change(
            1,
            OperatorInboxChangeKind::Upsert,
            item("proposal-1", 1, "one", ts(1)),
        );

        let result = store
            .apply_batch(origin, OperatorInboxCheckpoint::default(), &[action])
            .expect("apply");
        assert_eq!(result.checkpoint.current_sequence, 1);

        let candidates = store
            .notification_candidates(&open_notification_request(origin))
            .expect("candidates");
        assert_eq!(candidates.candidates.len(), 1);
        assert_eq!(
            candidates.candidates[0].status,
            OperatorNotificationCandidateStatus::Pending
        );
        assert_eq!(candidates.candidates[0].item.id, "proposal-1");
        assert_eq!(candidates.candidates[0].trigger_sequence, 1);
    }

    #[test]
    fn replayed_or_overlapping_batches_do_not_duplicate_candidates() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        let first = change(
            1,
            OperatorInboxChangeKind::Upsert,
            item("proposal-1", 1, "one", ts(1)),
        );
        let second = change(
            2,
            OperatorInboxChangeKind::Upsert,
            item("proposal-1", 2, "one-updated", ts(2)),
        );

        let result = store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[first.clone(), second.clone()],
            )
            .expect("apply");
        let repeat = store
            .apply_batch(
                origin,
                result.checkpoint.clone(),
                &[first.clone(), second.clone()],
            )
            .expect("repeat");
        assert_eq!(repeat.applied_changes, 0);
        let overlap = store
            .apply_batch(origin, result.checkpoint.clone(), &[second.clone()])
            .expect("overlap");
        assert_eq!(overlap.applied_changes, 0);

        let candidates = store
            .notification_candidates(&open_notification_request(origin))
            .expect("candidates");
        assert_eq!(candidates.candidates.len(), 1);
        assert_eq!(
            candidates.candidates[0].candidate_id,
            format!("{origin}::proposal-1::1")
        );
        assert_eq!(candidates.candidates[0].item.title, "one-updated");
    }

    #[test]
    fn terminal_transition_obsoletes_notification_candidate() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        let open = change(
            1,
            OperatorInboxChangeKind::Upsert,
            item("proposal-1", 1, "one", ts(1)),
        );
        let closed = change(
            2,
            OperatorInboxChangeKind::Upsert,
            resolved_item("proposal-1", 2, "one", ts(2)),
        );

        store
            .apply_batch(origin, OperatorInboxCheckpoint::default(), &[open, closed])
            .expect("apply");

        let candidate = store
            .notification_candidate(&OperatorNotificationGetRequest {
                origin_node_id: origin.to_string(),
                candidate_id: format!("{origin}::proposal-1::1"),
            })
            .expect("candidate")
            .expect("candidate present");
        assert_eq!(
            candidate.status,
            OperatorNotificationCandidateStatus::Obsolete
        );
        assert!(candidate.resolved_at.is_some());
        assert!(candidate.item.available_actions.is_empty());
    }

    #[test]
    fn reopened_item_creates_a_new_candidate_window() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        let open = change(
            1,
            OperatorInboxChangeKind::Upsert,
            item("proposal-1", 1, "one", ts(1)),
        );
        let closed = change(
            2,
            OperatorInboxChangeKind::Upsert,
            resolved_item("proposal-1", 2, "one", ts(2)),
        );
        let reopen = change(
            3,
            OperatorInboxChangeKind::Upsert,
            item("proposal-1", 3, "reopened", ts(3)),
        );

        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[open, closed, reopen],
            )
            .expect("apply");

        let candidates = store
            .notification_candidates(&open_notification_request(origin))
            .expect("candidates");
        assert_eq!(candidates.candidates.len(), 2);
        assert!(
            candidates
                .candidates
                .iter()
                .any(
                    |candidate| candidate.candidate_id == format!("{origin}::proposal-1::1")
                        && candidate.status == OperatorNotificationCandidateStatus::Obsolete
                )
        );
        assert!(
            candidates
                .candidates
                .iter()
                .any(
                    |candidate| candidate.candidate_id == format!("{origin}::proposal-1::3")
                        && candidate.status == OperatorNotificationCandidateStatus::Pending
                )
        );
    }

    #[test]
    fn passive_or_closed_items_do_not_create_candidates() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        let passive = OperatorInboxItem {
            available_actions: Vec::new(),
            status: OperatorInboxItemStatus::Resolved,
            ..item("proposal-1", 1, "one", ts(1))
        };
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(1, OperatorInboxChangeKind::Upsert, passive)],
            )
            .expect("apply");
        let candidates = store
            .notification_candidates(&open_notification_request(origin))
            .expect("candidates");
        assert!(candidates.candidates.is_empty());
    }

    #[test]
    fn non_actionable_items_do_not_create_remote_action_requests() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        let passive = OperatorInboxItem {
            available_actions: Vec::new(),
            status: OperatorInboxItemStatus::Resolved,
            ..item("proposal-1", 1, "one", ts(1))
        };
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(1, OperatorInboxChangeKind::Upsert, passive)],
            )
            .expect("apply");
        let error = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect_err("create should fail");
        let error = error.to_string();
        assert!(
            error.contains("not actionable") || error.contains("no actionable notification window")
        );
    }

    #[test]
    fn remote_action_request_creation_is_idempotent_for_the_same_window() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let first = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create");
        let second = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create duplicate");

        assert_eq!(first.request.request_id, second.request.request_id);
        assert_eq!(
            first.request.status,
            OperatorRemoteActionRequestStatus::Pending
        );
        assert_eq!(
            store
                .list_remote_action_requests(&remote_action_list_request(origin))
                .expect("list")
                .requests
                .len(),
            1
        );
    }

    #[test]
    fn remote_action_request_creation_is_idempotent_with_an_idempotency_key() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let first = store
            .create_remote_action_request(&remote_action_create_request_with_idempotency_key(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
                Some("client-retry-key"),
            ))
            .expect("create");
        let second = store
            .create_remote_action_request(&remote_action_create_request_with_idempotency_key(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
                Some("client-retry-key"),
            ))
            .expect("create duplicate");

        assert_eq!(first.request.request_id, second.request.request_id);
        assert_eq!(
            first.request.idempotency_key.as_deref(),
            Some("client-retry-key")
        );
        assert_eq!(
            second.request.idempotency_key.as_deref(),
            Some("client-retry-key")
        );
        assert_eq!(
            store
                .list_remote_action_requests(&remote_action_list_request(origin))
                .expect("list")
                .requests
                .len(),
            1
        );
    }

    #[test]
    fn remote_action_request_creation_without_an_idempotency_key_is_predictable() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let first = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create");
        let second = store
            .create_remote_action_request(&OperatorRemoteActionCreateRequest {
                origin_node_id: origin.to_string(),
                item_id: "proposal-1".to_string(),
                action_kind: OperatorInboxActionKind::Approve,
                idempotency_key: None,
                requested_by: Some("different-operator".to_string()),
                request_note: Some("retry without idempotency".to_string()),
            })
            .expect("create duplicate");

        assert_eq!(first.request.request_id, second.request.request_id);
        assert_eq!(first.request.idempotency_key, None);
        assert_eq!(second.request.idempotency_key, None);
        assert_eq!(
            store
                .list_remote_action_requests(&remote_action_list_request(origin))
                .expect("list")
                .requests
                .len(),
            1
        );
    }

    #[test]
    fn remote_action_claim_complete_survives_restart() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("server.db");
        let origin = "origin-a";
        let store = InboxMirrorStore::open(&path).expect("store");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");
        let created = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create");
        let claimed = store
            .claim_remote_action_requests(&remote_action_claim_request(origin, "worker-1"))
            .expect("claim");
        assert_eq!(claimed.requests.len(), 1);
        let claim = &claimed.requests[0];
        assert_eq!(claim.request.request_id, created.request.request_id);
        assert_eq!(
            claim.request.status,
            OperatorRemoteActionRequestStatus::Claimed
        );

        store
            .complete_remote_action_request(&remote_action_complete_request(
                origin,
                claim.request.request_id.as_str(),
                claim.claim_token.as_str(),
            ))
            .expect("complete");
        drop(store);

        let reopened = InboxMirrorStore::open(&path).expect("reopen");
        let request = reopened
            .get_remote_action_request(&remote_action_get_request(
                origin,
                created.request.request_id.as_str(),
            ))
            .expect("get")
            .request
            .expect("request");
        assert_eq!(request.status, OperatorRemoteActionRequestStatus::Completed);
        assert_eq!(request.result, Some(json!({"status": "ok"})));
    }

    #[test]
    fn pending_remote_action_requests_require_a_claim_token_to_complete_or_fail() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("server.db");
        let origin = "origin-a";
        let store = InboxMirrorStore::open(&path).expect("store");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");
        let created = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create");

        let complete_result =
            store.complete_remote_action_request(&remote_action_complete_request(
                origin,
                created.request.request_id.as_str(),
                "wrong-token",
            ));
        assert!(complete_result.is_err());

        let fail_result = store.fail_remote_action_request(&remote_action_fail_request(
            origin,
            created.request.request_id.as_str(),
            "wrong-token",
            "not allowed",
        ));
        assert!(fail_result.is_err());

        let request = store
            .get_remote_action_request(&remote_action_get_request(
                origin,
                created.request.request_id.as_str(),
            ))
            .expect("get")
            .request
            .expect("request");
        assert_eq!(request.status, OperatorRemoteActionRequestStatus::Pending);
        assert_eq!(request.claim_token, None);
    }

    #[test]
    fn remote_action_claim_lease_expiry_allows_reclaim_and_preserves_claim_ownership() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("server.db");
        let origin = "origin-a";
        let store = InboxMirrorStore::open(&path).expect("store");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");
        let created = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create");
        let first_claim = store
            .claim_remote_action_requests(&remote_action_claim_request_with_lease(
                origin, "worker-1", 1,
            ))
            .expect("claim");
        assert_eq!(first_claim.requests.len(), 1);
        let first = &first_claim.requests[0];
        std::thread::sleep(std::time::Duration::from_millis(5));
        drop(store);

        let reopened = InboxMirrorStore::open(&path).expect("reopen");
        let second_claim = reopened
            .claim_remote_action_requests(&remote_action_claim_request_with_lease(
                origin, "worker-2", 60_000,
            ))
            .expect("reclaim");
        assert_eq!(second_claim.requests.len(), 1);
        let second = &second_claim.requests[0];
        assert_eq!(second.request.request_id, created.request.request_id);
        assert_ne!(first.claim_token, second.claim_token);
        assert_eq!(second.request.claimed_by.as_deref(), Some("worker-2"));
        assert_eq!(
            second.request.status,
            OperatorRemoteActionRequestStatus::Claimed
        );

        let stale_complete =
            reopened.complete_remote_action_request(&remote_action_complete_request(
                origin,
                second.request.request_id.as_str(),
                first.claim_token.as_str(),
            ));
        assert!(stale_complete.is_err());

        reopened
            .complete_remote_action_request(&remote_action_complete_request(
                origin,
                second.request.request_id.as_str(),
                second.claim_token.as_str(),
            ))
            .expect("complete reclaimed request");
    }

    #[test]
    fn obsolete_candidates_mark_pending_remote_action_requests_stale() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply open");
        let created = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create");
        store
            .apply_batch(
                origin,
                store.checkpoint(origin).expect("checkpoint"),
                &[change(
                    2,
                    OperatorInboxChangeKind::Upsert,
                    resolved_item("proposal-1", 2, "one", ts(2)),
                )],
            )
            .expect("close");

        let request = store
            .get_remote_action_request(&remote_action_get_request(
                origin,
                created.request.request_id.as_str(),
            ))
            .expect("get")
            .request
            .expect("request");
        assert_eq!(request.status, OperatorRemoteActionRequestStatus::Stale);
        assert!(
            store
                .claim_remote_action_requests(&remote_action_claim_request(origin, "worker-1"))
                .expect("claim")
                .requests
                .is_empty()
        );
    }

    #[test]
    fn reopened_items_create_new_remote_action_windows() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply open");
        let first = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create first");
        store
            .apply_batch(
                origin,
                store.checkpoint(origin).expect("checkpoint"),
                &[change(
                    2,
                    OperatorInboxChangeKind::Upsert,
                    resolved_item("proposal-1", 2, "one", ts(2)),
                )],
            )
            .expect("close");
        store
            .apply_batch(
                origin,
                store.checkpoint(origin).expect("checkpoint reopen"),
                &[change(
                    3,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 3, "reopened", ts(3)),
                )],
            )
            .expect("reopen");
        let second = store
            .create_remote_action_request(&remote_action_create_request(
                origin,
                "proposal-1",
                OperatorInboxActionKind::Approve,
            ))
            .expect("create second");

        assert_ne!(first.request.request_id, second.request.request_id);
        let requests = store
            .list_remote_action_requests(&remote_action_list_request(origin))
            .expect("list")
            .requests;
        assert_eq!(requests.len(), 2);
    }

    #[test]
    fn pending_candidates_create_jobs_for_enabled_subscriptions() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";

        store
            .upsert_notification_recipient(&recipient_request("recipient-1", "Recipient 1", true))
            .expect("recipient");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        store
            .upsert_notification_subscription(&subscription_request(
                "subscription-1",
                "recipient-1",
                NotificationTransportKind::Mock,
                true,
            ))
            .expect("subscription 1");
        store
            .upsert_notification_subscription(&subscription_request(
                "subscription-2",
                "recipient-1",
                NotificationTransportKind::Log,
                true,
            ))
            .expect("subscription 2");

        let jobs = store
            .list_notification_delivery_jobs(&delivery_jobs_request(origin))
            .expect("jobs");
        assert_eq!(jobs.jobs.len(), 2);
        assert!(
            jobs.jobs
                .iter()
                .all(|job| job.status == NotificationDeliveryJobStatus::Pending)
        );
    }

    #[test]
    fn candidate_snapshot_updates_do_not_duplicate_delivery_jobs() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";

        store
            .upsert_notification_recipient(&recipient_request("recipient-1", "Recipient 1", true))
            .expect("recipient");
        store
            .upsert_notification_subscription(&subscription_request(
                "subscription-1",
                "recipient-1",
                NotificationTransportKind::Mock,
                true,
            ))
            .expect("subscription");

        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply first");
        store
            .apply_batch(
                origin,
                store.checkpoint(origin).expect("checkpoint"),
                &[change(
                    2,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 2, "one-updated", ts(2)),
                )],
            )
            .expect("apply update");

        let jobs = store
            .list_notification_delivery_jobs(&delivery_jobs_request(origin))
            .expect("jobs");
        assert_eq!(jobs.jobs.len(), 1);
        assert_eq!(
            jobs.jobs[0].candidate_id,
            format!("{origin}::proposal-1::1")
        );
        assert_eq!(jobs.jobs[0].status, NotificationDeliveryJobStatus::Pending);
    }

    #[test]
    fn acknowledged_candidates_suppress_pending_delivery_jobs() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";

        store
            .upsert_notification_recipient(&recipient_request("recipient-1", "Recipient 1", true))
            .expect("recipient");
        store
            .upsert_notification_subscription(&subscription_request(
                "subscription-1",
                "recipient-1",
                NotificationTransportKind::Mock,
                true,
            ))
            .expect("subscription");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let candidate_id = format!("{origin}::proposal-1::1");
        store
            .acknowledge_notification_candidate(&OperatorNotificationAckRequest {
                origin_node_id: origin.to_string(),
                candidate_id: candidate_id.clone(),
            })
            .expect("ack");

        let jobs = store
            .list_notification_delivery_jobs(&delivery_jobs_request(origin))
            .expect("jobs");
        assert_eq!(jobs.jobs.len(), 1);
        assert_eq!(
            jobs.jobs[0].status,
            NotificationDeliveryJobStatus::Suppressed
        );

        let pending = store
            .dispatch_pending_notification_delivery_jobs(
                &MockNotificationDeliveryTransport::default(),
                None,
            )
            .expect("dispatch");
        assert!(pending.jobs.is_empty());
    }

    #[test]
    fn reopened_items_create_new_delivery_windows() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";

        store
            .upsert_notification_recipient(&recipient_request("recipient-1", "Recipient 1", true))
            .expect("recipient");
        store
            .upsert_notification_subscription(&subscription_request(
                "subscription-1",
                "recipient-1",
                NotificationTransportKind::Mock,
                true,
            ))
            .expect("subscription");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[
                    change(
                        1,
                        OperatorInboxChangeKind::Upsert,
                        item("proposal-1", 1, "one", ts(1)),
                    ),
                    change(
                        2,
                        OperatorInboxChangeKind::Upsert,
                        resolved_item("proposal-1", 2, "one", ts(2)),
                    ),
                    change(
                        3,
                        OperatorInboxChangeKind::Upsert,
                        item("proposal-1", 3, "reopened", ts(3)),
                    ),
                ],
            )
            .expect("apply");

        let jobs = store
            .list_notification_delivery_jobs(&delivery_jobs_request(origin))
            .expect("jobs");
        assert_eq!(jobs.jobs.len(), 2);
        assert!(
            jobs.jobs
                .iter()
                .any(|job| job.status == NotificationDeliveryJobStatus::Obsolete
                    && job.trigger_sequence == 1)
        );
        assert!(
            jobs.jobs
                .iter()
                .any(|job| job.status == NotificationDeliveryJobStatus::Pending
                    && job.trigger_sequence == 3)
        );
    }

    #[test]
    fn disabled_subscriptions_do_not_receive_new_delivery_jobs() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";

        store
            .upsert_notification_recipient(&recipient_request("recipient-1", "Recipient 1", true))
            .expect("recipient");
        store
            .upsert_notification_subscription(&subscription_request(
                "subscription-1",
                "recipient-1",
                NotificationTransportKind::Mock,
                false,
            ))
            .expect("subscription");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let jobs = store
            .list_notification_delivery_jobs(&delivery_jobs_request(origin))
            .expect("jobs");
        assert!(jobs.jobs.is_empty());
    }

    #[test]
    fn mock_dispatch_updates_job_status_predictably() {
        let dir = tempdir().expect("tempdir");
        let store = InboxMirrorStore::open(dir.path().join("server.db")).expect("store");
        let origin = "origin-a";

        store
            .upsert_notification_recipient(&recipient_request("recipient-1", "Recipient 1", true))
            .expect("recipient");
        store
            .upsert_notification_subscription(&subscription_request(
                "subscription-1",
                "recipient-1",
                NotificationTransportKind::Mock,
                true,
            ))
            .expect("subscription");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let candidate_id = format!("{origin}::proposal-1::1");
        let job_id = format!("{origin}::{}::subscription-1::1", candidate_id);
        let transport = MockNotificationDeliveryTransport::default();
        transport.set_job_outcome(job_id.clone(), NotificationDeliveryOutcome::failed("boom"));
        let result = store
            .dispatch_pending_notification_delivery_jobs(&transport, None)
            .expect("dispatch");
        assert_eq!(result.jobs.len(), 1);
        assert_eq!(result.jobs[0].job_id, job_id);
        assert_eq!(result.jobs[0].status, NotificationDeliveryJobStatus::Failed);
        assert_eq!(result.jobs[0].attempt_count, 1);
        assert_eq!(result.jobs[0].error.as_deref(), Some("boom"));
    }

    #[test]
    fn delivery_jobs_persist_across_restart() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("server.db");
        let origin = "origin-a";

        let store = InboxMirrorStore::open(&path).expect("store");
        store
            .upsert_notification_recipient(&recipient_request("recipient-1", "Recipient 1", true))
            .expect("recipient");
        store
            .upsert_notification_subscription(&subscription_request(
                "subscription-1",
                "recipient-1",
                NotificationTransportKind::Mock,
                true,
            ))
            .expect("subscription");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");

        let candidate_id = format!("{origin}::proposal-1::1");
        let job_id = format!("{origin}::{}::subscription-1::1", candidate_id);
        let result = store
            .dispatch_pending_notification_delivery_jobs(
                &MockNotificationDeliveryTransport::with_job_outcome(
                    job_id.clone(),
                    NotificationDeliveryOutcome::delivered(Some(json!({
                        "persisted": true,
                    }))),
                ),
                None,
            )
            .expect("dispatch");
        assert_eq!(result.jobs.len(), 1);
        drop(store);

        let reopened = InboxMirrorStore::open(&path).expect("reopen");
        let jobs = reopened
            .list_notification_delivery_jobs(&delivery_jobs_request(origin))
            .expect("jobs");
        assert_eq!(jobs.jobs.len(), 1);
        assert_eq!(
            jobs.jobs[0].status,
            NotificationDeliveryJobStatus::Delivered
        );
        assert!(jobs.jobs[0].receipt.is_some());
    }

    #[test]
    fn candidate_ack_and_suppress_persist_across_restart() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("server.db");
        let origin = "origin-a";
        let store = InboxMirrorStore::open(&path).expect("store");
        store
            .apply_batch(
                origin,
                OperatorInboxCheckpoint::default(),
                &[change(
                    1,
                    OperatorInboxChangeKind::Upsert,
                    item("proposal-1", 1, "one", ts(1)),
                )],
            )
            .expect("apply");
        let candidate_id = format!("{origin}::proposal-1::1");
        let acked = store
            .acknowledge_notification_candidate(&OperatorNotificationAckRequest {
                origin_node_id: origin.to_string(),
                candidate_id: candidate_id.clone(),
            })
            .expect("ack");
        assert_eq!(
            acked.candidate.status,
            OperatorNotificationCandidateStatus::Acknowledged
        );
        let suppressed = store
            .suppress_notification_candidate(&OperatorNotificationSuppressRequest {
                origin_node_id: origin.to_string(),
                candidate_id: candidate_id.clone(),
            })
            .expect("suppress");
        assert_eq!(
            suppressed.candidate.status,
            OperatorNotificationCandidateStatus::Suppressed
        );
        drop(store);

        let reopened = InboxMirrorStore::open(&path).expect("reopen");
        let candidate = reopened
            .notification_candidate(&OperatorNotificationGetRequest {
                origin_node_id: origin.to_string(),
                candidate_id,
            })
            .expect("candidate")
            .expect("candidate present");
        assert_eq!(
            candidate.status,
            OperatorNotificationCandidateStatus::Suppressed
        );
        assert!(candidate.acknowledged_at.is_some());
        assert!(candidate.suppressed_at.is_some());
    }
}
