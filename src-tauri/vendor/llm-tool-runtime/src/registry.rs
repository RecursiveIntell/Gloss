use crate::{
    ToolDescriptor, ToolError, ToolErrorClass, ToolExposureMode, ToolExposurePolicy,
    ToolPlannerStage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[async_trait]
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> &ToolDescriptor;

    async fn invoke(
        &self,
        ctx: &crate::ToolCtx,
        call: &crate::ToolCall,
    ) -> Result<crate::ToolResult, ToolError>;
}

#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        self.tools
            .insert(tool.descriptor().name.clone(), Arc::new(tool));
    }

    /// Returns the registered tool implementation for the supplied name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Returns cloned descriptors for every registered tool.
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .values()
            .map(|tool| tool.descriptor().clone())
            .collect()
    }

    /// Computes which tools may be exposed for a planner request.
    pub fn plan_exposure(&self, request: &ToolExposureRequest) -> ToolExposurePlan {
        let mut allowed = Vec::new();
        let mut decisions = Vec::new();
        let allowed_names = request
            .allowed_names
            .as_ref()
            .map(|names| names.iter().cloned().collect::<BTreeSet<_>>());

        for tool in self.tools.values() {
            let descriptor = tool.descriptor();
            let mut reason = None;

            if descriptor.exposure_mode == ToolExposureMode::Hidden && !request.include_hidden {
                reason = Some("hidden".to_string());
            }

            if reason.is_none() {
                if let Some(ref allowed_name_set) = allowed_names {
                    if !allowed_name_set.contains(&descriptor.name) {
                        reason = Some("not_in_allowed_subset".to_string());
                    }
                } else if descriptor.exposure_mode == ToolExposureMode::OptIn {
                    reason = Some("opt_in".to_string());
                }
            }

            if reason.is_none()
                && !planner_stage_allowed(
                    &descriptor.exposure_policy,
                    request.planner_stage.clone(),
                )
            {
                reason = Some("planner_stage_blocked".to_string());
            }

            let exposed = reason.is_none();
            if exposed {
                allowed.push(tool.clone());
            }

            decisions.push(ToolExposureDecision {
                tool_name: descriptor.name.clone(),
                exposed,
                reason: reason.unwrap_or_else(|| "selected".to_string()),
            });
        }

        if let Some(limit) = request.max_tools {
            allowed.truncate(limit);
        }

        ToolExposurePlan {
            tools: allowed,
            decisions,
        }
    }
}

fn planner_stage_allowed(policy: &ToolExposurePolicy, stage: ToolPlannerStage) -> bool {
    if policy.allowed_planner_stages.is_empty() {
        return true;
    }

    policy.allowed_planner_stages.contains(&stage)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExposureRequest {
    pub allowed_names: Option<Vec<String>>,
    pub planner_stage: ToolPlannerStage,
    pub include_hidden: bool,
    pub max_tools: Option<usize>,
}

impl Default for ToolExposureRequest {
    fn default() -> Self {
        Self {
            allowed_names: None,
            planner_stage: ToolPlannerStage::Execution,
            include_hidden: false,
            max_tools: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExposureDecision {
    pub tool_name: String,
    pub exposed: bool,
    pub reason: String,
}

#[derive(Clone)]
pub struct ToolExposurePlan {
    pub tools: Vec<Arc<dyn Tool>>,
    pub decisions: Vec<ToolExposureDecision>,
}

/// Validates a JSON argument payload against a descriptor input schema.
pub fn validate_arguments_against_schema(
    schema: &Value,
    arguments: &Value,
) -> Result<(), ToolError> {
    validate_value(schema, arguments, "$")
        .map_err(|message| ToolError::new(ToolErrorClass::InvalidArguments, message))
}

#[allow(clippy::collapsible_match)]
fn validate_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(enum_values) = schema.get("enum").and_then(|value| value.as_array()) {
        if !enum_values.iter().any(|candidate| candidate == value) {
            return Err(format!("{path}: value is not in enum"));
        }
    }

    if let Some(expected_type) = schema.get("type").and_then(|value| value.as_str()) {
        match expected_type {
            "object" => validate_object(schema, value, path)?,
            "array" => validate_array(schema, value, path)?,
            "string" => {
                if !value.is_string() {
                    return Err(format!("{path}: expected string"));
                }
            }
            "number" => {
                if !value.is_number() {
                    return Err(format!("{path}: expected number"));
                }
            }
            "integer" => {
                if value.as_i64().is_none() && value.as_u64().is_none() {
                    return Err(format!("{path}: expected integer"));
                }
            }
            "boolean" => {
                if !value.is_boolean() {
                    return Err(format!("{path}: expected boolean"));
                }
            }
            "null" => {
                if !value.is_null() {
                    return Err(format!("{path}: expected null"));
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn validate_object(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{path}: expected object"))?;

    let required = schema
        .get("required")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    for required_key in required.iter().filter_map(|value| value.as_str()) {
        if !object.contains_key(required_key) {
            return Err(format!("{path}: missing required field {required_key}"));
        }
    }

    let properties = schema
        .get("properties")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();

    if schema
        .get("additionalProperties")
        .and_then(|value| value.as_bool())
        == Some(false)
    {
        for key in object.keys() {
            if !properties.contains_key(key) {
                return Err(format!("{path}: unexpected field {key}"));
            }
        }
    }

    for (key, property_schema) in properties {
        if let Some(field_value) = object.get(&key) {
            validate_value(&property_schema, field_value, &format!("{path}.{key}"))?;
        }
    }

    Ok(())
}

fn validate_array(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{path}: expected array"))?;

    if let Some(item_schema) = schema.get("items") {
        for (index, item) in items.iter().enumerate() {
            validate_value(item_schema, item, &format!("{path}[{index}]"))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_schema_validation_rejects_missing_required_fields() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {"type": "string"},
                "limit": {"type": "integer"}
            },
            "additionalProperties": false
        });

        let err = validate_arguments_against_schema(&schema, &serde_json::json!({"limit": 1}))
            .unwrap_err();
        assert!(err.message.contains("missing required field query"));
    }

    #[test]
    fn json_schema_validation_rejects_unknown_fields() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"}
            },
            "additionalProperties": false
        });

        let err = validate_arguments_against_schema(
            &schema,
            &serde_json::json!({"query": "ok", "extra": true}),
        )
        .unwrap_err();
        assert!(err.message.contains("unexpected field extra"));
    }
}
