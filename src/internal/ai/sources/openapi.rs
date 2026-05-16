use std::{collections::BTreeMap, fmt};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::internal::ai::{
    sources::SourceToolCapability,
    tools::spec::{FunctionDefinition, FunctionParameters, ToolSpec},
};

/// Build REST source tool declarations from a small OpenAPI v3 fixture.
///
/// This intentionally covers the fixture shape Source Pool needs first:
/// operation ids, operation/path parameters, and JSON request bodies. It
/// produces tool specs only; a concrete REST source is responsible for turning
/// accepted tool calls into HTTP requests.
pub fn openapi_tool_capabilities_from_fixture(
    fixture: &str,
) -> Result<Vec<SourceToolCapability>, OpenApiToolSpecError> {
    let document: OpenApiDocument =
        serde_json::from_str(fixture).map_err(OpenApiToolSpecError::InvalidJson)?;
    let mut capabilities = Vec::new();

    for (path, item) in document.paths {
        let path_parameters = &item.parameters;
        for (method, operation) in item.operations() {
            let tool_name = operation
                .operation_id
                .as_deref()
                .map(sanitise_tool_name)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| sanitise_tool_name(&format!("{method}_{path}")));
            if tool_name.is_empty() {
                return Err(OpenApiToolSpecError::InvalidOperationName {
                    method: method.to_string(),
                    path,
                });
            }

            let description = operation_description(method, &path, operation);
            let mut properties = Map::new();
            let mut required = Vec::new();
            for parameter in path_parameters.iter().chain(operation.parameters.iter()) {
                properties.insert(parameter.name.clone(), parameter_schema(parameter));
                if parameter.required.unwrap_or(false) {
                    push_required(&mut required, &parameter.name);
                }
            }

            if let Some(request_body) = &operation.request_body
                && let Some(schema) = request_body.json_schema()
            {
                properties.insert("body".to_string(), schema);
                if request_body.required.unwrap_or(false) {
                    push_required(&mut required, "body");
                }
            }

            let parameters = if properties.is_empty() {
                FunctionParameters::Empty
            } else {
                FunctionParameters::Object {
                    param_type: "object".to_string(),
                    properties,
                    required,
                    definitions: None,
                }
            };
            let spec = ToolSpec {
                spec_type: "function".to_string(),
                function: FunctionDefinition {
                    name: tool_name.clone(),
                    description,
                    parameters,
                },
            };
            capabilities.push(SourceToolCapability::new(tool_name, spec).with_network(true));
        }
    }

    if capabilities.is_empty() {
        return Err(OpenApiToolSpecError::NoOperations);
    }
    capabilities.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(capabilities)
}

#[derive(Debug)]
pub enum OpenApiToolSpecError {
    InvalidJson(serde_json::Error),
    InvalidOperationName { method: String, path: String },
    NoOperations,
}

impl fmt::Display for OpenApiToolSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(f, "failed to parse OpenAPI fixture: {error}"),
            Self::InvalidOperationName { method, path } => write!(
                f,
                "OpenAPI operation {method} {path} did not produce a valid tool name"
            ),
            Self::NoOperations => write!(f, "OpenAPI fixture did not contain any operations"),
        }
    }
}

impl std::error::Error for OpenApiToolSpecError {}

#[derive(Debug, Deserialize)]
struct OpenApiDocument {
    #[serde(default)]
    paths: BTreeMap<String, PathItem>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathItem {
    #[serde(default)]
    parameters: Vec<OpenApiParameter>,
    get: Option<OpenApiOperation>,
    post: Option<OpenApiOperation>,
    put: Option<OpenApiOperation>,
    patch: Option<OpenApiOperation>,
    delete: Option<OpenApiOperation>,
}

impl PathItem {
    fn operations(&self) -> Vec<(&'static str, &OpenApiOperation)> {
        [
            ("get", self.get.as_ref()),
            ("post", self.post.as_ref()),
            ("put", self.put.as_ref()),
            ("patch", self.patch.as_ref()),
            ("delete", self.delete.as_ref()),
        ]
        .into_iter()
        .filter_map(|(method, operation)| operation.map(|operation| (method, operation)))
        .collect()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenApiOperation {
    operation_id: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    #[serde(default)]
    parameters: Vec<OpenApiParameter>,
    request_body: Option<OpenApiRequestBody>,
}

#[derive(Debug, Deserialize)]
struct OpenApiParameter {
    name: String,
    #[serde(rename = "in")]
    location: String,
    required: Option<bool>,
    description: Option<String>,
    #[serde(default)]
    schema: Value,
}

#[derive(Debug, Deserialize)]
struct OpenApiRequestBody {
    required: Option<bool>,
    #[serde(default)]
    content: BTreeMap<String, OpenApiMediaType>,
}

impl OpenApiRequestBody {
    fn json_schema(&self) -> Option<Value> {
        self.content
            .get("application/json")
            .or_else(|| self.content.get("application/*+json"))
            .map(|media| media.schema.clone())
    }
}

#[derive(Debug, Deserialize)]
struct OpenApiMediaType {
    #[serde(default = "default_object_schema")]
    schema: Value,
}

fn default_object_schema() -> Value {
    json!({ "type": "object" })
}

fn parameter_schema(parameter: &OpenApiParameter) -> Value {
    let mut schema = if parameter.schema.is_null() {
        json!({ "type": "string" })
    } else {
        parameter.schema.clone()
    };
    let description = parameter
        .description
        .clone()
        .unwrap_or_else(|| format!("{} parameter `{}`", parameter.location, parameter.name));
    if let Value::Object(ref mut object) = schema {
        object
            .entry("description".to_string())
            .or_insert(Value::String(description));
    }
    schema
}

fn operation_description(method: &str, path: &str, operation: &OpenApiOperation) -> String {
    operation
        .summary
        .as_deref()
        .or(operation.description.as_deref())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("Call {method} {path}"))
}

fn sanitise_tool_name(raw: &str) -> String {
    let mut name = String::new();
    let mut previous_was_separator = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && !name.is_empty() && !previous_was_separator {
                name.push('_');
            }
            name.push(ch.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !name.is_empty() && !previous_was_separator {
            name.push('_');
            previous_was_separator = true;
        }
    }
    while name.ends_with('_') {
        name.pop();
    }
    while name.contains("__") {
        name = name.replace("__", "_");
    }
    name
}

fn push_required(required: &mut Vec<String>, name: &str) {
    if !required.iter().any(|entry| entry == name) {
        required.push(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::OpenApiToolSpecError;

    #[test]
    fn openapi_tool_spec_error_display_pins_owned_variants() {
        assert_eq!(
            OpenApiToolSpecError::InvalidOperationName {
                method: "GET".to_string(),
                path: "/users".to_string(),
            }
            .to_string(),
            "OpenAPI operation GET /users did not produce a valid tool name",
        );
        assert_eq!(
            OpenApiToolSpecError::NoOperations.to_string(),
            "OpenAPI fixture did not contain any operations",
        );

        let invalid_json = OpenApiToolSpecError::InvalidJson(
            serde_json::from_str::<serde_json::Value>("not json").unwrap_err(),
        );
        let rendered = invalid_json.to_string();
        assert!(
            rendered.starts_with("failed to parse OpenAPI fixture: "),
            "got: {rendered}",
        );
    }
}
