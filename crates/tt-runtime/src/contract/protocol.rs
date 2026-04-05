use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ContractError;
use super::read_json;
use super::schema::{SchemaContract, load_schema_contract};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolContract {
    pub client_request_schema: SchemaContract,
    pub client_notification_schema: SchemaContract,
    pub server_request_schema: SchemaContract,
    pub server_notification_schema: SchemaContract,
    pub client_requests: Vec<ProtocolMethod>,
    pub client_notifications: Vec<ProtocolMethod>,
    pub server_requests: Vec<ProtocolMethod>,
    pub server_notifications: Vec<ProtocolMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolMethod {
    pub method: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub params_ref: Option<String>,
    pub experimental: bool,
}

pub fn load_contract(root: impl AsRef<Path>) -> Result<ProtocolContract, ContractError> {
    let root = root.as_ref();
    let client_request_path = root.join("app-server-protocol/schema/json/ClientRequest.json");
    let client_notification_path =
        root.join("app-server-protocol/schema/json/ClientNotification.json");
    let server_request_path = root.join("app-server-protocol/schema/json/ServerRequest.json");
    let server_notification_path =
        root.join("app-server-protocol/schema/json/ServerNotification.json");

    let client_request_value = read_json(&client_request_path)?;
    let client_notification_value = read_json(&client_notification_path)?;
    let server_request_value = read_json(&server_request_path)?;
    let server_notification_value = read_json(&server_notification_path)?;

    Ok(ProtocolContract {
        client_request_schema: load_schema_contract(client_request_path)?,
        client_notification_schema: load_schema_contract(client_notification_path)?,
        server_request_schema: load_schema_contract(server_request_path)?,
        server_notification_schema: load_schema_contract(server_notification_path)?,
        client_requests: extract_methods(&client_request_value),
        client_notifications: extract_methods(&client_notification_value),
        server_requests: extract_methods(&server_request_value),
        server_notifications: extract_methods(&server_notification_value),
    })
}

impl ProtocolContract {
    pub fn method_names(&self) -> Vec<String> {
        let mut methods = Vec::new();
        methods.extend(
            self.client_requests
                .iter()
                .map(|method| format!("client_request:{}", method.method)),
        );
        methods.extend(
            self.client_notifications
                .iter()
                .map(|method| format!("client_notification:{}", method.method)),
        );
        methods.extend(
            self.server_requests
                .iter()
                .map(|method| format!("server_request:{}", method.method)),
        );
        methods.extend(
            self.server_notifications
                .iter()
                .map(|method| format!("server_notification:{}", method.method)),
        );
        methods.sort();
        methods.dedup();
        methods
    }
}

fn extract_methods(schema: &Value) -> Vec<ProtocolMethod> {
    schema
        .get("oneOf")
        .and_then(Value::as_array)
        .map(|variants| {
            variants
                .iter()
                .map(|variant| {
                    let method = variant
                        .get("properties")
                        .and_then(Value::as_object)
                        .and_then(|properties| properties.get("method"))
                        .and_then(Value::as_object)
                        .and_then(|method| method.get("enum"))
                        .and_then(Value::as_array)
                        .and_then(|values| values.first())
                        .and_then(Value::as_str)
                        .unwrap_or("<unknown>")
                        .to_string();
                    let title = variant
                        .get("title")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let description = variant
                        .get("description")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let params_ref = variant
                        .get("properties")
                        .and_then(Value::as_object)
                        .and_then(|properties| properties.get("params"))
                        .and_then(Value::as_object)
                        .and_then(|params| params.get("$ref"))
                        .and_then(Value::as_str)
                        .map(|reference| {
                            reference
                                .rsplit('/')
                                .next()
                                .unwrap_or(reference)
                                .to_string()
                        });
                    let experimental = description
                        .as_deref()
                        .map(|description| {
                            description.contains("experimental")
                                || description.contains("NEW APIs")
                                || description.contains("NEW NOTIFICATIONS")
                        })
                        .unwrap_or(false);
                    ProtocolMethod {
                        method,
                        title,
                        description,
                        params_ref,
                        experimental,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
