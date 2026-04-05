use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderValue};
use tracing::debug;

use tt_core::ipc::{
    OperatorRemoteActionClaimRequest, OperatorRemoteActionClaimResponse,
    OperatorRemoteActionCompleteRequest, OperatorRemoteActionCompleteResponse,
    OperatorRemoteActionCreateRequest, OperatorRemoteActionCreateResponse,
    OperatorRemoteActionFailRequest, OperatorRemoteActionFailResponse,
    OperatorRemoteActionGetRequest, OperatorRemoteActionGetResponse,
    OperatorRemoteActionListRequest, OperatorRemoteActionListResponse,
};
use tt_core::{TTError, TTResult};

#[derive(Debug, Clone)]
pub struct RemoteActionHttpClient {
    client: Client,
    base_url: String,
    operator_api_token: Option<String>,
}

impl RemoteActionHttpClient {
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

    pub async fn create(
        &self,
        request: &OperatorRemoteActionCreateRequest,
    ) -> TTResult<OperatorRemoteActionCreateResponse> {
        let response = self
            .authorized_request(self.client.post(self.url("operator-actions/request")))
            .json(request)
            .send()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| TTError::Transport(error.to_string()))?
            .json::<OperatorRemoteActionCreateResponse>()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?;
        Ok(response)
    }

    pub async fn list(
        &self,
        request: &OperatorRemoteActionListRequest,
    ) -> TTResult<OperatorRemoteActionListResponse> {
        let response = self
            .authorized_request(self.client.post(self.url("operator-actions/list")))
            .json(request)
            .send()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| TTError::Transport(error.to_string()))?
            .json::<OperatorRemoteActionListResponse>()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?;
        Ok(response)
    }

    pub async fn get(
        &self,
        request: &OperatorRemoteActionGetRequest,
    ) -> TTResult<OperatorRemoteActionGetResponse> {
        let response = self
            .authorized_request(self.client.post(self.url("operator-actions/get")))
            .json(request)
            .send()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| TTError::Transport(error.to_string()))?
            .json::<OperatorRemoteActionGetResponse>()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?;
        Ok(response)
    }

    pub async fn claim(
        &self,
        request: &OperatorRemoteActionClaimRequest,
    ) -> TTResult<OperatorRemoteActionClaimResponse> {
        let response = self
            .authorized_request(self.client.post(self.url("operator-actions/claim")))
            .json(request)
            .send()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| TTError::Transport(error.to_string()))?
            .json::<OperatorRemoteActionClaimResponse>()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?;
        debug!(
            origin_node_id = %response.origin_node_id,
            claimed = response.requests.len(),
            "remote action claims fetched"
        );
        Ok(response)
    }

    pub async fn complete(
        &self,
        request: &OperatorRemoteActionCompleteRequest,
    ) -> TTResult<OperatorRemoteActionCompleteResponse> {
        let response = self
            .authorized_request(self.client.post(self.url("operator-actions/complete")))
            .json(request)
            .send()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| TTError::Transport(error.to_string()))?
            .json::<OperatorRemoteActionCompleteResponse>()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?;
        Ok(response)
    }

    pub async fn fail(
        &self,
        request: &OperatorRemoteActionFailRequest,
    ) -> TTResult<OperatorRemoteActionFailResponse> {
        let response = self
            .authorized_request(self.client.post(self.url("operator-actions/fail")))
            .json(request)
            .send()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| TTError::Transport(error.to_string()))?
            .json::<OperatorRemoteActionFailResponse>()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?;
        Ok(response)
    }

    pub async fn wait(
        &self,
        request: &tt_core::ipc::OperatorRemoteActionWaitRequest,
    ) -> TTResult<tt_core::ipc::OperatorRemoteActionWaitResponse> {
        let response = self
            .authorized_request(self.client.post(self.url("operator-actions/wait")))
            .json(request)
            .send()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| TTError::Transport(error.to_string()))?
            .json::<tt_core::ipc::OperatorRemoteActionWaitResponse>()
            .await
            .map_err(|error| TTError::Transport(error.to_string()))?;
        Ok(response)
    }
}
