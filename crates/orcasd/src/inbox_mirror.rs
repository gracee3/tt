use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use tracing::debug;

use orcas_core::ipc::{
    OperatorInboxMirrorApplyRequest, OperatorInboxMirrorApplyResponse,
    OperatorInboxMirrorCheckpointQueryRequest, OperatorInboxMirrorCheckpointQueryResponse,
    OperatorInboxMirrorGetResponse, OperatorInboxMirrorListResponse,
};
use orcas_core::{OrcasError, OrcasResult};

#[derive(Debug, Clone)]
pub struct OperatorInboxMirrorHttpClient {
    client: Client,
    base_url: String,
    operator_api_token: Option<String>,
}

impl OperatorInboxMirrorHttpClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            operator_api_token: None,
        }
    }

    pub fn with_operator_api_token(
        base_url: impl Into<String>,
        operator_api_token: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            operator_api_token: Some(operator_api_token.into()),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    fn authorized_request(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(token) = &self.operator_api_token {
            let value = format!("Bearer {token}");
            if let Ok(header_value) = HeaderValue::from_str(&value) {
                return builder.header(AUTHORIZATION, header_value);
            }
        }
        builder
    }

    pub async fn checkpoint(
        &self,
        origin_node_id: &str,
    ) -> OrcasResult<OperatorInboxMirrorCheckpointQueryResponse> {
        let request = OperatorInboxMirrorCheckpointQueryRequest {
            origin_node_id: origin_node_id.to_string(),
        };
        let response = self
            .authorized_request(self.client.get(self.url(&format!(
                "operator-inbox/{}/checkpoint",
                request.origin_node_id
            ))))
            .send()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .json::<OperatorInboxMirrorCheckpointQueryResponse>()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        debug!(
            origin_node_id,
            sequence = response.checkpoint.current_sequence,
            "mirror checkpoint fetched"
        );
        Ok(response)
    }

    pub async fn apply(
        &self,
        request: &OperatorInboxMirrorApplyRequest,
    ) -> OrcasResult<OperatorInboxMirrorApplyResponse> {
        let response = self
            .authorized_request(self.client.post(self.url("operator-inbox/mirror/apply")))
            .json(request)
            .send()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .json::<OperatorInboxMirrorApplyResponse>()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        Ok(response)
    }

    pub async fn list(&self, origin_node_id: &str) -> OrcasResult<OperatorInboxMirrorListResponse> {
        let response = self
            .authorized_request(
                self.client
                    .get(self.url(&format!("operator-inbox/{origin_node_id}/items"))),
            )
            .send()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .json::<OperatorInboxMirrorListResponse>()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        Ok(response)
    }

    pub async fn get(
        &self,
        origin_node_id: &str,
        item_id: &str,
    ) -> OrcasResult<OperatorInboxMirrorGetResponse> {
        let response = self
            .authorized_request(
                self.client
                    .get(self.url(&format!("operator-inbox/{origin_node_id}/items/{item_id}"))),
            )
            .send()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| OrcasError::Transport(error.to_string()))?
            .json::<OperatorInboxMirrorGetResponse>()
            .await
            .map_err(|error| OrcasError::Transport(error.to_string()))?;
        Ok(response)
    }
}
