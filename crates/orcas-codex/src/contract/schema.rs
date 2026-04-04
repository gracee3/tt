use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ContractError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaContract {
    pub root: SchemaNode,
    pub definitions: Vec<SchemaDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDefinition {
    pub name: String,
    pub node: SchemaNode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaNode {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub kind: SchemaKind,
    pub ref_name: Option<String>,
    pub default: Option<Value>,
    pub enum_values: Vec<String>,
    pub required: Vec<String>,
    pub additional_properties: Option<Box<SchemaNode>>,
    pub properties: Vec<SchemaNode>,
    pub one_of: Vec<SchemaNode>,
    pub any_of: Vec<SchemaNode>,
    pub items: Option<Box<SchemaNode>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaKind {
    Any,
    Object,
    Array,
    String,
    Integer,
    Number,
    Boolean,
    Null,
    Enum,
    OneOf,
    AnyOf,
    Ref,
}

pub fn load_schema_contract(
    path: impl AsRef<std::path::Path>,
) -> Result<SchemaContract, ContractError> {
    let path = path.as_ref();
    let value = super::read_json(path)?;
    schema_contract_from_value(&value)
}

pub fn schema_contract_from_value(value: &Value) -> Result<SchemaContract, ContractError> {
    let definitions = value
        .get("definitions")
        .and_then(Value::as_object)
        .map(|definitions| {
            definitions
                .iter()
                .map(|(name, def)| SchemaDefinition {
                    name: name.clone(),
                    node: schema_node_from_value(name, def),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let root_name = value
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Schema");

    Ok(SchemaContract {
        root: schema_node_from_value(root_name, value),
        definitions,
    })
}

fn schema_node_from_value(name: &str, value: &Value) -> SchemaNode {
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let default = value.get("default").cloned();
    let ref_name = value.get("$ref").and_then(Value::as_str).map(|reference| {
        reference
            .rsplit('/')
            .next()
            .unwrap_or(reference)
            .to_string()
    });

    let kind = if ref_name.is_some() {
        SchemaKind::Ref
    } else if value.get("oneOf").is_some() {
        SchemaKind::OneOf
    } else if value.get("anyOf").is_some() {
        SchemaKind::AnyOf
    } else if value
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty == "object")
        .unwrap_or(false)
    {
        SchemaKind::Object
    } else if value
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty == "array")
        .unwrap_or(false)
    {
        SchemaKind::Array
    } else if value
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty == "string")
        .unwrap_or(false)
        || value.get("enum").is_some()
    {
        if value.get("enum").is_some() {
            SchemaKind::Enum
        } else {
            SchemaKind::String
        }
    } else if value
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty == "integer")
        .unwrap_or(false)
    {
        SchemaKind::Integer
    } else if value
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty == "number")
        .unwrap_or(false)
    {
        SchemaKind::Number
    } else if value
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty == "boolean")
        .unwrap_or(false)
    {
        SchemaKind::Boolean
    } else if value
        .get("type")
        .and_then(Value::as_str)
        .map(|ty| ty == "null")
        .unwrap_or(false)
    {
        SchemaKind::Null
    } else {
        SchemaKind::Any
    };

    let enum_values = value
        .get("enum")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let required = value
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let properties = value
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(field, node)| schema_node_from_value(field, node))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let one_of = value
        .get("oneOf")
        .and_then(Value::as_array)
        .map(|variants| {
            variants
                .iter()
                .enumerate()
                .map(|(index, variant)| {
                    let variant_name = variant
                        .get("title")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("{name}_{index}"));
                    schema_node_from_value(&variant_name, variant)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let any_of = value
        .get("anyOf")
        .and_then(Value::as_array)
        .map(|variants| {
            variants
                .iter()
                .enumerate()
                .map(|(index, variant)| {
                    let variant_name = variant
                        .get("title")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| format!("{name}_{index}"));
                    schema_node_from_value(&variant_name, variant)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let items = value
        .get("items")
        .map(|item| Box::new(schema_node_from_value(&format!("{name}_item"), item)));

    let additional_properties = match value.get("additionalProperties") {
        Some(Value::Bool(true)) => Some(Box::new(SchemaNode {
            name: format!("{name}.*"),
            title: None,
            description: None,
            kind: SchemaKind::Any,
            ref_name: None,
            default: None,
            enum_values: Vec::new(),
            required: Vec::new(),
            additional_properties: None,
            properties: Vec::new(),
            one_of: Vec::new(),
            any_of: Vec::new(),
            items: None,
        })),
        Some(Value::Object(object)) => Some(Box::new(schema_node_from_value(
            &format!("{name}.*"),
            &Value::Object(object.clone()),
        ))),
        _ => None,
    };

    SchemaNode {
        name: name.to_string(),
        title,
        description,
        kind,
        ref_name,
        default,
        enum_values,
        required,
        additional_properties,
        properties,
        one_of,
        any_of,
        items,
    }
}
