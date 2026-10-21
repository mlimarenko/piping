//! NetBox resource registry: descriptors, field metadata, filter parameters and supported operations for each known resource.
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::errors::AppError;

#[derive(Debug, Clone, Serialize)]
pub struct FilterParam {
    pub param_type: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldChoice {
    pub value: Value,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldMeta {
    pub name: String,
    pub field_type: String,
    pub required: bool,
    pub read_only: bool,
    pub description: String,
    pub max_length: Option<Value>,
    pub minimum: Option<Value>,
    pub maximum: Option<Value>,
    pub pattern: Option<String>,
    pub choices: Option<Vec<FieldChoice>>,
    pub ref_resource_key: Option<String>,
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetboxResourceDescriptor {
    pub resource_key: String,
    pub list_path: String,
    pub detail_path_template: String,
    pub operations_supported: HashSet<String>,
    pub filter_params: HashMap<String, FilterParam>,
    pub ordering_supported: bool,
    pub fields_meta: Vec<FieldMeta>,
    pub changelog_message_supported: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetboxResourceMeta {
    pub resource_key: String,
    pub fields: Vec<FieldMeta>,
    pub operations: Vec<String>,
    pub filters: HashMap<String, FilterParam>,
    pub relationships: Option<HashMap<String, String>>,
    pub list_path: String,
    pub detail_path_template: String,
    pub ordering_supported: bool,
    pub changelog_message_supported: bool,
}

#[derive(Debug, Clone)]
struct PathSelection {
    list_path_key: String,
    detail_path_key: String,
    has_detail_candidates: bool,
}

#[derive(Debug, Clone)]
struct OperationSummary {
    operations_supported: HashSet<String>,
    filter_params: HashMap<String, FilterParam>,
    ordering_supported: bool,
}

impl NetboxResourceDescriptor {
    pub fn supports_operation(&self, operation: &str) -> bool {
        self.operations_supported.contains(operation)
    }

    pub fn get_detail_path(&self, resource_id: &str) -> String {
        self.detail_path_template
            .replace("{id}", resource_id)
            .replace("{pk}", resource_id)
    }
}

#[derive(Debug)]
pub struct NetboxResourceRegistry {
    schema: Value,
    descriptors: HashMap<String, NetboxResourceDescriptor>,
    schema_base_name_to_resource_key: HashMap<String, String>,
    resource_response_schema_name: HashMap<String, String>,
}

impl NetboxResourceRegistry {
    pub fn from_schema(schema: Value) -> Result<Self, AppError> {
        if schema.get("paths").and_then(Value::as_object).is_none() {
            return Err(AppError::Validation(
                "NetBox schema missing 'paths' section".to_string(),
            ));
        }

        let mut registry = Self {
            schema,
            descriptors: HashMap::new(),
            schema_base_name_to_resource_key: HashMap::new(),
            resource_response_schema_name: HashMap::new(),
        };
        registry.build_descriptors();
        Ok(registry)
    }

    /// Construct a registry with no resources. Used for provider types that do
    /// not expose a discoverable schema (e.g. the Custom provider).
    pub fn empty() -> Self {
        Self {
            schema: Value::Object(Map::new()),
            descriptors: HashMap::new(),
            schema_base_name_to_resource_key: HashMap::new(),
            resource_response_schema_name: HashMap::new(),
        }
    }

    pub fn get_descriptor(&self, resource_key: &str) -> Option<&NetboxResourceDescriptor> {
        self.descriptors.get(resource_key)
    }

    pub fn list_resources(&self) -> Vec<String> {
        let mut resources: Vec<String> = self.descriptors.keys().cloned().collect();
        resources.sort();
        resources
    }

    pub fn has_resource(&self, resource_key: &str) -> bool {
        self.descriptors.contains_key(resource_key)
    }

    pub fn get_resource_meta(&self, resource_key: &str) -> Option<NetboxResourceMeta> {
        let descriptor = self.get_descriptor(resource_key)?;
        let mut operations: Vec<String> = descriptor.operations_supported.iter().cloned().collect();
        operations.sort();
        let mut relationships = HashMap::new();
        for field in &descriptor.fields_meta {
            if let Some(ref_resource_key) = field.ref_resource_key.as_ref() {
                relationships.insert(field.name.clone(), ref_resource_key.clone());
            }
        }
        Some(NetboxResourceMeta {
            resource_key: resource_key.to_string(),
            fields: descriptor.fields_meta.clone(),
            operations,
            filters: descriptor.filter_params.clone(),
            relationships: if relationships.is_empty() {
                None
            } else {
                Some(relationships)
            },
            list_path: descriptor.list_path.clone(),
            detail_path_template: descriptor.detail_path_template.clone(),
            ordering_supported: descriptor.ordering_supported,
            changelog_message_supported: descriptor.changelog_message_supported,
        })
    }
}

mod builders;
mod schema_introspection;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sample_schema() -> Value {
        json!({
            "paths": {
                "/api/dcim/sites/": {
                    "get": {
                        "parameters": [
                            { "name": "ordering", "in": "query", "schema": { "type": "string" } },
                            { "name": "name", "in": "query", "schema": { "type": "string" } }
                        ],
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": "#/components/schemas/SiteList" }
                                    }
                                }
                            }
                        }
                    },
                    "post": {
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/SiteRequest" }
                                }
                            }
                        }
                    },
                    "patch": {
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": { "type": "array" }
                                }
                            }
                        }
                    }
                },
                "/api/dcim/sites/{id}/": {
                    "get": {
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": "#/components/schemas/Site" }
                                    }
                                }
                            }
                        }
                    },
                    "delete": {}
                }
            },
            "components": {
                "schemas": {
                    "SiteRequest": {
                        "type": "object",
                        "required": ["name"],
                        "properties": {
                            "name": { "type": "string" },
                            "status": {
                                "type": "string",
                                "enum": ["active", "planned"],
                                "description": "* `active` - Active\n* `planned` - Planned"
                            }
                        }
                    },
                    "Site": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer", "readOnly": true },
                            "name": { "type": "string" },
                            "status": { "type": "string" }
                        }
                    },
                    "SiteList": {
                        "type": "object",
                        "properties": {
                            "results": {
                                "type": "array",
                                "items": { "$ref": "#/components/schemas/Site" }
                            }
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn registry_builds_descriptor_and_operations() {
        let registry = NetboxResourceRegistry::from_schema(sample_schema()).expect("registry");
        assert!(registry.has_resource("dcim.sites"));
        let descriptor = registry.get_descriptor("dcim.sites").expect("descriptor");
        assert_eq!(descriptor.list_path, "api/dcim/sites/");
        assert_eq!(descriptor.get_detail_path("42"), "api/dcim/sites/42/");
        assert!(descriptor.supports_operation("list"));
        assert!(descriptor.supports_operation("create"));
        assert!(descriptor.supports_operation("bulk_update"));
        assert!(descriptor.supports_operation("get"));
        assert!(descriptor.supports_operation("delete"));
        assert!(descriptor.ordering_supported);
        assert!(descriptor.filter_params.contains_key("name"));
    }

    #[test]
    fn registry_extracts_field_choices_from_enum_description() {
        let registry = NetboxResourceRegistry::from_schema(sample_schema()).expect("registry");
        let descriptor = registry.get_descriptor("dcim.sites").expect("descriptor");
        let status = descriptor
            .fields_meta
            .iter()
            .find(|field| field.name == "status")
            .expect("status field");
        let choices = status.choices.as_ref().expect("choices");
        let labels: Vec<_> = choices.iter().map(|choice| choice.label.as_str()).collect();
        assert!(labels.contains(&"Active"));
        assert!(labels.contains(&"Planned"));
        assert!(status.description.is_empty());
    }
}
