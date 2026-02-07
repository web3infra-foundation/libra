#[test]
fn test_part_deserialization() {
    use crate::internal::ai::providers::gemini::gemini_api_types::{Part, PartKind};

    // Test text part
    let json = r#"{"text": "Hello, world!"}"#;
    let part: Part = serde_json::from_str(json).unwrap();
    if let PartKind::Text(text) = part.part {
        assert_eq!(text, "Hello, world!");
    } else {
        panic!("Expected Text part, got {:?}", part);
    }

    // Test function call part
    let json = r#"{"functionCall": {"name": "get_weather", "args": {"location": "London"}}}"#;
    let part: Part = serde_json::from_str(json).unwrap();
    if let PartKind::FunctionCall(fc) = part.part {
        assert_eq!(fc.name, "get_weather");
        assert_eq!(fc.args["location"], "London");
    } else {
        panic!("Expected FunctionCall part, got {:?}", part);
    }

    // Test function response part
    let json = r#"{"functionResponse": {"name": "get_weather", "response": {"result": "Sunny"}}}"#;
    let part: Part = serde_json::from_str(json).unwrap();
    if let PartKind::FunctionResponse(fr) = part.part {
        assert_eq!(fr.name, "get_weather");
        assert_eq!(fr.response.unwrap()["result"], "Sunny");
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
