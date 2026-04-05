use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, warn};

use tt_core::{ReconnectPolicy, TTError, TTResult};

pub struct TransportConnection {
    pub outbound: mpsc::Sender<String>,
    pub inbound: mpsc::Receiver<String>,
}

#[async_trait]
pub trait TTTransport: Send + Sync {
    async fn connect(&self) -> TTResult<TransportConnection>;
    fn endpoint(&self) -> &str;
}

#[derive(Debug, Clone)]
pub struct WebSocketTransport {
    endpoint: String,
}

impl WebSocketTransport {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}

#[async_trait]
impl TTTransport for WebSocketTransport {
    async fn connect(&self) -> TTResult<TransportConnection> {
        let (socket, response) = connect_async(&self.endpoint)
            .await
            .map_err(|error| TTError::Transport(format!("websocket connect failed: {error}")))?;
        debug!(status = ?response.status(), endpoint = %self.endpoint, "connected websocket transport");

        let (mut write, mut read) = socket.split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(256);
        let (inbound_tx, inbound_rx) = mpsc::channel::<String>(256);
        let endpoint = self.endpoint.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    outbound = outbound_rx.recv() => {
                        match outbound {
                            Some(payload) => {
                                if let Err(error) = write.send(Message::Text(payload.into())).await {
                                    warn!(endpoint = %endpoint, %error, "websocket send failed");
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    inbound = read.next() => {
                        match inbound {
                            Some(Ok(Message::Text(text))) => {
                                if inbound_tx.send(text.to_string()).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(Message::Ping(payload))) => {
                                if let Err(error) = write.send(Message::Pong(payload)).await {
                                    warn!(endpoint = %endpoint, %error, "failed to answer websocket ping");
                                    break;
                                }
                            }
                            Some(Ok(Message::Pong(_))) | Some(Ok(Message::Binary(_))) | Some(Ok(Message::Frame(_))) => {}
                            Some(Ok(Message::Close(frame))) => {
                                debug!(endpoint = %endpoint, ?frame, "websocket transport closed");
                                break;
                            }
                            Some(Err(error)) => {
                                warn!(endpoint = %endpoint, %error, "websocket receive failed");
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        Ok(TransportConnection {
            outbound: outbound_tx,
            inbound: inbound_rx,
        })
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

#[derive(Debug, Clone)]
pub struct ReconnectBackoff {
    policy: ReconnectPolicy,
}

impl ReconnectBackoff {
    pub fn new(policy: ReconnectPolicy) -> Self {
        Self { policy }
    }

    pub fn delay_for_attempt(&self, attempt: u32) -> std::time::Duration {
        let scaled = (self.policy.initial_delay_ms as f64)
            * self
                .policy
                .multiplier
                .powi(i32::try_from(attempt).unwrap_or(i32::MAX));
        let delay_ms = scaled.min(self.policy.max_delay_ms as f64) as u64;
        std::time::Duration::from_millis(delay_ms)
    }

    pub fn should_retry(&self, attempt: u32) -> bool {
        self.policy
            .max_attempts
            .is_none_or(|max_attempts| attempt < max_attempts)
    }
}

#[cfg(test)]
mod tests {
    use tt_core::ReconnectPolicy;

    use super::ReconnectBackoff;

    #[test]
    fn backoff_caps_at_max_delay() {
        let backoff = ReconnectBackoff::new(ReconnectPolicy {
            initial_delay_ms: 100,
            max_delay_ms: 500,
            multiplier: 3.0,
            max_attempts: Some(3),
        });

        assert_eq!(backoff.delay_for_attempt(0).as_millis(), 100);
        assert_eq!(backoff.delay_for_attempt(1).as_millis(), 300);
        assert_eq!(backoff.delay_for_attempt(2).as_millis(), 500);
        assert!(!backoff.should_retry(3));
    }
}
