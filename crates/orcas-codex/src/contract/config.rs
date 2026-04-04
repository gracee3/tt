use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::ContractError;
use super::schema::{SchemaContract, load_schema_contract};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigContract {
    pub schema: SchemaContract,
}

pub fn load_contract(root: impl AsRef<Path>) -> Result<ConfigContract, ContractError> {
    let schema_path = root.as_ref().join("core/config.schema.json");
    Ok(ConfigContract {
        schema: load_schema_contract(schema_path)?,
    })
}

impl ConfigContract {
    pub fn key_paths(&self) -> Vec<String> {
        let defs = definition_map(&self.schema);
        let mut paths = Vec::new();
        walk_schema_node("", &self.schema.root, &defs, &mut paths);
        paths.sort();
        paths.dedup();
        paths
    }
}

fn definition_map(schema: &SchemaContract) -> BTreeMap<String, &super::schema::SchemaNode> {
    schema
        .definitions
        .iter()
        .map(|definition| (definition.name.clone(), &definition.node))
        .collect()
}

fn walk_schema_node(
    prefix: &str,
    node: &super::schema::SchemaNode,
    defs: &BTreeMap<String, &super::schema::SchemaNode>,
    paths: &mut Vec<String>,
) {
    if !prefix.is_empty() {
        paths.push(prefix.to_string());
    }

    let resolved = match node.ref_name.as_ref().and_then(|name| defs.get(name)) {
        Some(resolved) => Some(*resolved),
        None => None,
    };
    let node = resolved.unwrap_or(node);

    for child in &node.properties {
        let child_path = if prefix.is_empty() {
            child.name.clone()
        } else {
            format!("{prefix}.{}", child.name)
        };
        walk_schema_node(&child_path, child, defs, paths);
    }

    for child in &node.one_of {
        walk_schema_node(prefix, child, defs, paths);
    }

    for child in &node.any_of {
        walk_schema_node(prefix, child, defs, paths);
    }

    if let Some(items) = &node.items {
        walk_schema_node(prefix, items, defs, paths);
    }

    if let Some(additional) = &node.additional_properties {
        walk_schema_node(prefix, additional, defs, paths);
    }
}
