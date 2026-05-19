//! Unit tests for Gemini API type serialization and deserialization.

#[test]
fn test_part_deserialization() {
    use crate::internal::ai::providers::gemini::gemini_api_types::Part;

    // Test text part
    let json = r#"{"text": "Hello, world!"}"#;
    let part: Part = serde_json::from_str(json).unwrap();
    if let Some(text) = part.text {
        assert_eq!(text, "Hello, world!");
    } else {
        panic!("Expected Text part, got {:?}", part);
    }

    // Test function call part
    let json = r#"{"functionCall": {"name": "get_weather", "args": {"location": "London"}}}"#;
    let part: Part = serde_json::from_str(json).unwrap();
    if let Some(fc) = part.function_call {
        assert_eq!(fc.name, "get_weather");
        assert_eq!(fc.args["location"], "London");
    } else {
        panic!("Expected FunctionCall part, got {:?}", part);
    }

    // Test function response part
    let json = r#"{"functionResponse": {"name": "get_weather", "response": {"result": "Sunny"}}}"#;
    let part: Part = serde_json::from_str(json).unwrap();
    if let Some(fr) = part.function_response {
        assert_eq!(fr.name, "get_weather");
        assert_eq!(fr.response["result"], "Sunny");
    } else {
        panic!("Expected FunctionResponse part, got {:?}", part);
    }
}

#[test]
fn test_tool_definition_generation() {
    use std::error::Error;

    use serde_json::Value;

    use crate::internal::ai::tools::{Tool, ToolDefinition};

    struct MyTool;
    impl Tool for MyTool {
        fn name(&self) -> String {
            "my_tool".to_string()
        }
        fn description(&self) -> String {
            "My Tool Description".to_string()
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name(),
                description: self.description(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "param1": {"type": "string"}
                    }
                }),
            }
        }
        fn call(&self, _args: Value) -> Result<Value, Box<dyn Error + Send + Sync>> {
            Ok(Value::Null)
        }
    }

    let tool = MyTool;
    let def = tool.definition();
    assert_eq!(def.name, "my_tool");
    assert_eq!(def.description, "My Tool Description");
    assert_eq!(def.parameters["type"], "object");
}

// ---------------------------------------------------------------------
// OC-Phase 4 P4.2 — wire-level quirk tests
// ---------------------------------------------------------------------

/// Quirk: Gemini accepts a system prompt only via the request-body
/// `systemInstruction` field (camelCase on the wire because of the
/// struct-level `rename_all = "camelCase"`). The preamble must serialise
/// there, NOT as a regular content entry. Pin the wire shape so a future
/// rename of the field name or a serde attribute regression breaks the
/// test before the API does.
#[test]
fn quirk_system_instruction_serialises_as_camelcase_field() {
    use crate::internal::ai::providers::gemini::gemini_api_types::{
        Content, GenerateContentRequest, Part,
    };

    let body = GenerateContentRequest {
        contents: vec![Content {
            role: Some("user".to_string()),
            parts: vec![Part::text("hi")],
        }],
        system_instruction: Some(Content::text("you are helpful")),
        generation_config: None,
        tools: None,
    };
    let json = serde_json::to_value(&body).unwrap();
    // Field name on the wire is `systemInstruction` (camelCase), not
    // `system_instruction`.
    assert!(
        json.get("systemInstruction").is_some(),
        "Gemini wire must carry `systemInstruction` (camelCase), got {json}",
    );
    assert!(
        json.get("system_instruction").is_none(),
        "snake_case form must not appear on the wire, got {json}",
    );
    // The system instruction itself is a Content with a single text part.
    let parts = json["systemInstruction"]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["text"], "you are helpful");
}

/// Quirk: when no preamble is supplied, the `systemInstruction` field is
/// omitted entirely from the wire (skipped by serde). Gemini rejects an
/// empty `systemInstruction.parts` array; the omitted-when-None path is
/// the only safe shape.
#[test]
fn quirk_system_instruction_omitted_when_none() {
    use crate::internal::ai::providers::gemini::gemini_api_types::{
        Content, GenerateContentRequest, Part,
    };

    let body = GenerateContentRequest {
        contents: vec![Content {
            role: Some("user".to_string()),
            parts: vec![Part::text("hi")],
        }],
        system_instruction: None,
        generation_config: None,
        tools: None,
    };
    let json = serde_json::to_value(&body).unwrap();
    assert!(
        json.get("systemInstruction").is_none(),
        "systemInstruction must be omitted when None, got {json}",
    );
}
