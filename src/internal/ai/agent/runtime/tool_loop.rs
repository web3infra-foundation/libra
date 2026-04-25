use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use serde_json::Value;

use crate::internal::ai::{
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionReasoningEffort,
        CompletionRequest, CompletionStreamEvent, CompletionThinking, CompletionUsage,
        CompletionUsageSummary, Message, OneOrMany, ToolResult, UserContent,
    },
    hooks::{HookAction, HookRunner},
    tools::{
        FunctionParameters, ToolDefinition, ToolInvocation, ToolOutput, ToolPayload, ToolRegistry,
        ToolRuntimeContext,
    },
};

/// A single complete tool-loop turn result.
#[derive(Clone, Debug)]
pub struct ToolLoopTurn {
    pub final_text: String,
    pub history: Vec<Message>,
}

/// Observer hooks for tool-loop execution.
///
/// All callbacks are best-effort and must be non-panicking.
pub trait ToolLoopObserver: Send {
    fn on_model_turn_start(&mut self, _turn: usize) {}

    fn on_model_usage(&mut self, _usage: &CompletionUsageSummary) {}

    fn on_model_stream_event(&mut self, _event: &CompletionStreamEvent) {}

    fn on_assistant_step_text(&mut self, _text: &str) {}

    fn on_tool_call_begin(&mut self, _call_id: &str, _tool_name: &str, _arguments: &Value) {}

    fn on_tool_call_preflight(
        &mut self,
        _call_id: &str,
        _tool_name: &str,
        _arguments: &Value,
    ) -> Result<(), String> {
        Ok(())
    }

    fn on_tool_call_end(
        &mut self,
        _call_id: &str,
        _tool_name: &str,
        _result: &Result<ToolOutput, String>,
    ) {
    }
}

struct NoopObserver;

impl ToolLoopObserver for NoopObserver {}

/// Runtime configuration for iterative tool-calling execution.
#[derive(Clone, Debug)]
pub struct ToolLoopConfig {
    pub preamble: Option<String>,
    pub temperature: Option<f64>,
    pub thinking: Option<CompletionThinking>,
    pub reasoning_effort: Option<CompletionReasoningEffort>,
    pub stream: Option<bool>,
    /// Optional hook runner for pre/post tool-use hooks.
    pub hook_runner: Option<Arc<HookRunner>>,
    /// If set, only expose these tools to the model (agent tool restriction).
    pub allowed_tools: Option<Vec<String>>,
    /// Optional runtime constraints injected into every tool invocation.
    pub runtime_context: Option<ToolRuntimeContext>,
    /// Hard cap for model turns in one tool loop run.
    pub max_turns: Option<usize>,
    /// Number of recent executed tool calls used to detect repeated calls.
    pub repeat_detection_window: Option<usize>,
    /// Same executed tool-call signature count that triggers a strategy warning.
    pub repeat_warning_threshold: Option<usize>,
    /// Whether assistant reasoning content should be retained in model history.
    pub preserve_reasoning_content: bool,
}

impl Default for ToolLoopConfig {
    fn default() -> Self {
        Self {
            preamble: None,
            temperature: Some(0.0),
            thinking: None,
            reasoning_effort: None,
            stream: None,
            hook_runner: None,
            allowed_tools: None,
            runtime_context: None,
            max_turns: None,
            repeat_detection_window: Some(DEFAULT_REPEAT_DETECTION_WINDOW),
            repeat_warning_threshold: Some(DEFAULT_REPEAT_WARNING_THRESHOLD),
            preserve_reasoning_content: false,
        }
    }
}

const DEFAULT_MAX_TOOL_LOOP_TURNS: usize = 64;
const DEFAULT_REPEAT_DETECTION_WINDOW: usize = 10;
const DEFAULT_REPEAT_WARNING_THRESHOLD: usize = 3;
const MAX_IDENTICAL_BLOCKED_TOOL_CALLS: usize = 3;

/// Run a prompt through a completion model, allowing iterative tool calls.
pub async fn run_tool_loop<M: CompletionModel>(
    model: &M,
    prompt: impl Into<String>,
    registry: &ToolRegistry,
    config: ToolLoopConfig,
) -> Result<String, CompletionError>
where
    M::Response: CompletionUsage,
{
    let mut observer = NoopObserver;
    let turn = run_tool_loop_with_history_and_observer(
        model,
        Vec::new(),
        prompt,
        registry,
        config,
        &mut observer,
    )
    .await?;
    Ok(turn.final_text)
}

/// Run a prompt through a completion model with an existing conversation history,
/// allowing iterative tool calls and emitting observer callbacks.
pub async fn run_tool_loop_with_history_and_observer<M: CompletionModel, O: ToolLoopObserver>(
    model: &M,
    mut existing_history: Vec<Message>,
    prompt: impl Into<String>,
    registry: &ToolRegistry,
    config: ToolLoopConfig,
    observer: &mut O,
) -> Result<ToolLoopTurn, CompletionError>
where
    M::Response: CompletionUsage,
{
    existing_history.push(Message::user(prompt.into()));
    let mut history = existing_history;
    let max_turns = config.max_turns.unwrap_or(DEFAULT_MAX_TOOL_LOOP_TURNS);
    if max_turns == 0 {
        return Err(CompletionError::ResponseError(
            "Tool loop max_turns must be greater than 0".to_string(),
        ));
    }
    let mut turn_count = 0usize;
    let mut blocked_signatures: HashMap<String, usize> = HashMap::new();
    let repeat_detection_window = config
        .repeat_detection_window
        .unwrap_or(DEFAULT_REPEAT_DETECTION_WINDOW);
    let repeat_warning_threshold = config
        .repeat_warning_threshold
        .unwrap_or(DEFAULT_REPEAT_WARNING_THRESHOLD);
    let mut executed_tool_signatures: VecDeque<String> = VecDeque::new();
    let mut executed_tool_signature_counts: HashMap<String, usize> = HashMap::new();

    let mut tools = registry_tool_definitions(registry);

    // Apply agent tool restriction
    if let Some(ref allowed) = config.allowed_tools {
        tools.retain(|t| allowed.iter().any(|a| a == &t.name));
    }

    loop {
        if turn_count >= max_turns {
            return Err(CompletionError::ResponseError(format!(
                "Tool loop exceeded maximum turns ({max_turns})"
            )));
        }
        turn_count += 1;

        let (stream_tx, mut stream_rx) = tokio::sync::mpsc::unbounded_channel();
        let request = CompletionRequest {
            preamble: config.preamble.clone(),
            chat_history: history.clone(),
            temperature: config.temperature,
            thinking: config.thinking,
            reasoning_effort: config.reasoning_effort,
            stream: config.stream,
            tools: tools.clone(),
            stream_events: Some(stream_tx),
            ..Default::default()
        };

        observer.on_model_turn_start(turn_count);
        let response = {
            let completion = model.completion(request);
            tokio::pin!(completion);

            loop {
                tokio::select! {
                    result = &mut completion => {
                        while let Ok(event) = stream_rx.try_recv() {
                            observer.on_model_stream_event(&event);
                        }
                        break result?;
                    }
                    Some(event) = stream_rx.recv() => {
                        observer.on_model_stream_event(&event);
                    }
                }
            }
        };
        if let Some(usage) = response.raw_response.usage_summary() {
            observer.on_model_usage(&usage);
        }

        let mut tool_calls = Vec::new();
        let mut text_parts = Vec::new();
        for content in &response.content {
            match content {
                AssistantContent::ToolCall(call) => tool_calls.push(call.clone()),
                AssistantContent::Text(text) => {
                    if !text.text.trim().is_empty() {
                        text_parts.push(text.text.clone());
                    }
                }
            }
        }

        if !tool_calls.is_empty() {
            if !text_parts.is_empty() {
                observer.on_assistant_step_text(&text_parts.join("\n"));
            }

            let assistant_content = OneOrMany::many(response.content.clone()).ok_or_else(|| {
                CompletionError::ResponseError(
                    "Empty assistant content in tool call response".to_string(),
                )
            })?;
            history.push(Message::Assistant {
                id: None,
                reasoning_content: history_reasoning_content(
                    response.reasoning_content.clone(),
                    config.preserve_reasoning_content,
                ),
                content: assistant_content,
            });

            for call in tool_calls {
                observer.on_tool_call_begin(
                    &call.id,
                    &call.function.name,
                    &call.function.arguments,
                );

                if let Err(reason) = observer.on_tool_call_preflight(
                    &call.id,
                    &call.function.name,
                    &call.function.arguments,
                ) {
                    let blocked_result: Result<ToolOutput, String> = Err(reason.clone());
                    observer.on_tool_call_end(&call.id, &call.function.name, &blocked_result);

                    let result_json = ToolOutput::failure(reason).into_response();
                    history.push(Message::User {
                        content: OneOrMany::One(UserContent::ToolResult(ToolResult {
                            id: call.id,
                            name: call.function.name.clone(),
                            result: result_json,
                        })),
                    });
                    let signature =
                        blocked_tool_call_signature(&call.function.name, &call.function.arguments);
                    if increment_blocked_count(&mut blocked_signatures, &signature)
                        >= MAX_IDENTICAL_BLOCKED_TOOL_CALLS
                    {
                        return Err(CompletionError::ResponseError(format!(
                            "Tool loop aborted after repeated blocked calls to '{}' with identical arguments",
                            call.function.name
                        )));
                    }
                    continue;
                }

                // Run PreToolUse hooks (may block the tool call)
                if let Some(ref hook_runner) = config.hook_runner {
                    let action = hook_runner
                        .run_pre_tool_use(&call.function.name, call.function.arguments.clone())
                        .await;
                    if let HookAction::Block(reason) = action {
                        let blocked_result: Result<ToolOutput, String> =
                            Err(format!("Blocked by hook: {reason}"));
                        observer.on_tool_call_end(&call.id, &call.function.name, &blocked_result);

                        let result_json = ToolOutput::failure(format!("Blocked by hook: {reason}"))
                            .into_response();

                        history.push(Message::User {
                            content: OneOrMany::One(UserContent::ToolResult(ToolResult {
                                id: call.id,
                                name: call.function.name.clone(),
                                result: result_json,
                            })),
                        });
                        let signature = blocked_tool_call_signature(
                            &call.function.name,
                            &call.function.arguments,
                        );
                        if increment_blocked_count(&mut blocked_signatures, &signature)
                            >= MAX_IDENTICAL_BLOCKED_TOOL_CALLS
                        {
                            return Err(CompletionError::ResponseError(format!(
                                "Tool loop aborted after repeated blocked calls to '{}' with identical arguments",
                                call.function.name
                            )));
                        }
                        continue;
                    }
                }

                // Enforce allowed_tools at execution time (not just definition filtering)
                if let Some(ref allowed) = config.allowed_tools
                    && !allowed.iter().any(|a| a == &call.function.name)
                {
                    let blocked_msg = format!(
                        "Tool '{}' is not in the allowed_tools list for this agent",
                        call.function.name
                    );
                    let blocked_result: Result<ToolOutput, String> = Err(blocked_msg.clone());
                    observer.on_tool_call_end(&call.id, &call.function.name, &blocked_result);

                    let result_json = ToolOutput::failure(blocked_msg).into_response();

                    history.push(Message::User {
                        content: OneOrMany::One(UserContent::ToolResult(ToolResult {
                            id: call.id,
                            name: call.function.name.clone(),
                            result: result_json,
                        })),
                    });
                    let signature =
                        blocked_tool_call_signature(&call.function.name, &call.function.arguments);
                    if increment_blocked_count(&mut blocked_signatures, &signature)
                        >= MAX_IDENTICAL_BLOCKED_TOOL_CALLS
                    {
                        return Err(CompletionError::ResponseError(format!(
                            "Tool loop aborted after repeated blocked calls to '{}' with identical arguments",
                            call.function.name
                        )));
                    }
                    continue;
                }

                let mut invocation = ToolInvocation::new(
                    call.id.clone(),
                    call.function.name.clone(),
                    ToolPayload::Function {
                        arguments: tool_arguments_json(&call.function.arguments),
                    },
                    registry.working_dir().to_path_buf(),
                );
                if let Some(runtime_context) = config.runtime_context.clone() {
                    invocation = invocation.with_runtime_context(runtime_context);
                }

                let mut tool_result: Result<ToolOutput, String> =
                    match registry.dispatch(invocation).await {
                        Ok(output) => Ok(output),
                        Err(err) => Err(format!("Tool '{}' failed: {}", call.function.name, err)),
                    };
                blocked_signatures.clear();
                let repeat_warning = record_executed_tool_signature(
                    &mut executed_tool_signatures,
                    &mut executed_tool_signature_counts,
                    &call.function.name,
                    &call.function.arguments,
                    repeat_detection_window,
                    repeat_warning_threshold,
                );
                if let Some(warning) = repeat_warning {
                    append_repeat_warning_to_tool_result(&mut tool_result, &warning);
                }

                observer.on_tool_call_end(&call.id, &call.function.name, &tool_result);

                // Run PostToolUse hooks
                if let Some(ref hook_runner) = config.hook_runner {
                    let output_json = match &tool_result {
                        Ok(output) => output.clone().into_response(),
                        Err(msg) => serde_json::json!({"error": msg}),
                    };
                    hook_runner
                        .run_post_tool_use(
                            &call.function.name,
                            call.function.arguments.clone(),
                            output_json,
                        )
                        .await;
                }

                let result_json = match &tool_result {
                    Ok(output) => output.clone().into_response(),
                    Err(message) => ToolOutput::failure(message.clone()).into_response(),
                };

                history.push(Message::User {
                    content: OneOrMany::One(UserContent::ToolResult(ToolResult {
                        id: call.id,
                        name: call.function.name,
                        result: result_json,
                    })),
                });
            }

            continue;
        }

        let final_text = text_parts.join("\n");
        if !final_text.trim().is_empty() {
            let assistant_content = OneOrMany::many(response.content.clone()).ok_or_else(|| {
                CompletionError::ResponseError("Empty assistant text response".to_string())
            })?;
            history.push(Message::Assistant {
                id: None,
                reasoning_content: history_reasoning_content(
                    response.reasoning_content.clone(),
                    config.preserve_reasoning_content,
                ),
                content: assistant_content,
            });
            return Ok(ToolLoopTurn {
                final_text,
                history,
            });
        }

        if !response.content.is_empty() {
            return Err(CompletionError::ResponseError(
                "Model returned non-text response (likely only thought or unsupported content)"
                    .to_string(),
            ));
        }
    }
}

fn tool_arguments_json(arguments: &Value) -> String {
    match arguments {
        Value::String(raw) => {
            if serde_json::from_str::<Value>(raw).is_ok() {
                raw.clone()
            } else {
                arguments.to_string()
            }
        }
        _ => arguments.to_string(),
    }
}

fn blocked_tool_call_signature(tool_name: &str, arguments: &Value) -> String {
    format!("{tool_name}|{}", canonical_json_value(arguments))
}

fn increment_blocked_count(
    blocked_signatures: &mut HashMap<String, usize>,
    signature: &str,
) -> usize {
    let count = blocked_signatures.entry(signature.to_string()).or_insert(0);
    *count += 1;
    *count
}

fn history_reasoning_content(
    reasoning_content: Option<String>,
    preserve_reasoning_content: bool,
) -> Option<String> {
    if preserve_reasoning_content {
        reasoning_content
    } else {
        None
    }
}

fn record_executed_tool_signature(
    recent_signatures: &mut VecDeque<String>,
    signature_counts: &mut HashMap<String, usize>,
    tool_name: &str,
    arguments: &Value,
    window: usize,
    threshold: usize,
) -> Option<String> {
    if window == 0 || threshold == 0 {
        return None;
    }

    let signature = format!("{tool_name}|{}", canonical_json_value(arguments));
    recent_signatures.push_back(signature.clone());
    *signature_counts.entry(signature.clone()).or_insert(0) += 1;

    while recent_signatures.len() > window {
        if let Some(expired) = recent_signatures.pop_front()
            && let Some(count) = signature_counts.get_mut(&expired)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                signature_counts.remove(&expired);
            }
        }
    }

    let count = signature_counts
        .get(&signature)
        .copied()
        .unwrap_or_default();
    (count >= threshold).then(|| {
        format!(
            "[system warning: repeated tool call] You have called `{tool_name}` with the same arguments {count} times in the last {window} executed tool calls. Do not repeat the same call again unless new information changed the target; switch strategy or finish with a final response if the task is complete."
        )
    })
}

fn append_repeat_warning_to_tool_result(result: &mut Result<ToolOutput, String>, warning: &str) {
    match result {
        Ok(ToolOutput::Function { content, .. }) => {
            if !content.is_empty() {
                content.push_str("\n\n");
            }
            content.push_str(warning);
        }
        Ok(ToolOutput::Mcp { result }) => {
            if let Value::Object(object) = result {
                object.insert(
                    "repeat_warning".to_string(),
                    Value::String(warning.to_string()),
                );
            }
        }
        Err(message) => {
            if !message.is_empty() {
                message.push_str("\n\n");
            }
            message.push_str(warning);
        }
    }
}

fn canonical_json_value(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value.to_string(),
        Value::Array(items) => {
            let inner = items
                .iter()
                .map(canonical_json_value)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{inner}]")
        }
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            let inner = entries
                .into_iter()
                .map(|(key, value)| {
                    let key = match serde_json::to_string(key) {
                        Ok(serialized) => serialized,
                        Err(_) => "\"<invalid-key>\"".to_string(),
                    };
                    format!("{key}:{}", canonical_json_value(value))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{inner}}}")
        }
    }
}

fn registry_tool_definitions(registry: &ToolRegistry) -> Vec<ToolDefinition> {
    registry
        .tool_specs()
        .into_iter()
        .map(|spec| {
            let parameters = match spec.function.parameters {
                FunctionParameters::Empty => serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
                params => serde_json::to_value(params).unwrap_or_else(|_| {
                    serde_json::json!({
                        "type": "object",
                        "properties": {}
                    })
                }),
            };
            ToolDefinition {
                name: spec.function.name,
                description: spec.function.description,
                parameters,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;
    use crate::internal::ai::{
        completion::{
            CompletionResponse,
            message::{Function, Text, ToolCall},
        },
        tools::{ToolHandler, ToolKind, ToolSpec},
    };

    #[derive(Clone)]
    struct MockModel;

    impl CompletionModel for MockModel {
        type Response = ();

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let has_tool_result = request.chat_history.iter().any(|msg| match msg {
                Message::User { content } => content
                    .iter()
                    .any(|c| matches!(c, UserContent::ToolResult(_))),
                _ => false,
            });

            if !has_tool_result {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "mock_tool".to_string(),
                        function: Function {
                            name: "mock_tool".to_string(),
                            arguments: json!({"value": 1}),
                        },
                    })],
                    reasoning_content: None,
                    raw_response: (),
                });
            }

            Ok(CompletionResponse {
                content: vec![AssistantContent::Text(Text {
                    text: "done".to_string(),
                })],
                reasoning_content: None,
                raw_response: (),
            })
        }
    }

    struct MockHandler;

    #[async_trait]
    impl ToolHandler for MockHandler {
        fn kind(&self) -> ToolKind {
            ToolKind::Function
        }

        async fn handle(
            &self,
            _invocation: ToolInvocation,
        ) -> crate::internal::ai::tools::ToolResult<ToolOutput> {
            Ok(ToolOutput::success("ok"))
        }

        fn schema(&self) -> ToolSpec {
            ToolSpec::new("mock_tool", "mock tool")
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        begins: Vec<(String, String)>,
        ends: Vec<(String, String, bool)>,
        stream_events: Vec<CompletionStreamEvent>,
    }

    impl ToolLoopObserver for RecordingObserver {
        fn on_model_stream_event(&mut self, event: &CompletionStreamEvent) {
            self.stream_events.push(event.clone());
        }

        fn on_tool_call_begin(&mut self, call_id: &str, tool_name: &str, _arguments: &Value) {
            self.begins
                .push((call_id.to_string(), tool_name.to_string()));
        }

        fn on_tool_call_end(
            &mut self,
            call_id: &str,
            tool_name: &str,
            result: &Result<ToolOutput, String>,
        ) {
            self.ends.push((
                call_id.to_string(),
                tool_name.to_string(),
                result.as_ref().is_ok_and(|o| o.is_success()),
            ));
        }
    }

    #[tokio::test]
    async fn tool_loop_forwards_completion_stream_events() {
        #[derive(Clone)]
        struct StreamingModel;

        impl CompletionModel for StreamingModel {
            type Response = ();

            async fn completion(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                if let Some(stream_events) = request.stream_events {
                    stream_events
                        .send(CompletionStreamEvent::TextDelta {
                            request_id: Some("req_1".to_string()),
                            delta: "hel".to_string(),
                        })
                        .expect("stream receiver should be open");
                    stream_events
                        .send(CompletionStreamEvent::ThinkingDelta {
                            request_id: Some("req_1".to_string()),
                            delta: "checking".to_string(),
                        })
                        .expect("stream receiver should be open");
                    stream_events
                        .send(CompletionStreamEvent::TextDelta {
                            request_id: Some("req_1".to_string()),
                            delta: "lo".to_string(),
                        })
                        .expect("stream receiver should be open");
                }

                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "hello".to_string(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }

        let temp_dir = TempDir::new().unwrap();
        let registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        let mut observer = RecordingObserver::default();

        let turn = run_tool_loop_with_history_and_observer(
            &StreamingModel,
            Vec::new(),
            "hello",
            &registry,
            ToolLoopConfig::default(),
            &mut observer,
        )
        .await
        .unwrap();

        assert_eq!(turn.final_text, "hello");
        assert_eq!(observer.stream_events.len(), 3);
        assert!(observer.stream_events.iter().any(|event| matches!(
            event,
            CompletionStreamEvent::ThinkingDelta { delta, .. } if delta == "checking"
        )));
        let streamed = observer
            .stream_events
            .iter()
            .filter_map(|event| match event {
                CompletionStreamEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                CompletionStreamEvent::ThinkingDelta { .. } => None,
                CompletionStreamEvent::ToolCallPreview { .. } => None,
            })
            .collect::<String>();
        assert_eq!(streamed, "hello");
    }

    #[tokio::test]
    async fn tool_loop_forwards_tool_call_preview_without_execution() {
        #[derive(Clone)]
        struct ToolPreviewModel;

        impl CompletionModel for ToolPreviewModel {
            type Response = ();

            async fn completion(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                if let Some(stream_events) = request.stream_events {
                    stream_events
                        .send(CompletionStreamEvent::ToolCallPreview {
                            request_id: Some("req_1".to_string()),
                            call_id: "call_preview".to_string(),
                            tool_name: "mock_tool".to_string(),
                            arguments: json!({"value": 1}),
                        })
                        .expect("stream receiver should be open");
                }

                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "done".to_string(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }

        let temp_dir = TempDir::new().unwrap();
        let registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        let mut observer = RecordingObserver::default();

        let turn = run_tool_loop_with_history_and_observer(
            &ToolPreviewModel,
            Vec::new(),
            "hello",
            &registry,
            ToolLoopConfig::default(),
            &mut observer,
        )
        .await
        .unwrap();

        assert_eq!(turn.final_text, "done");
        assert!(observer.begins.is_empty());
        assert_eq!(observer.stream_events.len(), 1);
        match &observer.stream_events[0] {
            CompletionStreamEvent::ToolCallPreview {
                call_id,
                tool_name,
                arguments,
                ..
            } => {
                assert_eq!(call_id, "call_preview");
                assert_eq!(tool_name, "mock_tool");
                assert_eq!(arguments, &json!({"value": 1}));
            }
            other => panic!("expected tool preview event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_loop_emits_tool_events_and_updates_history() {
        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        let mut observer = RecordingObserver::default();
        let turn = run_tool_loop_with_history_and_observer(
            &MockModel,
            Vec::new(),
            "hello",
            &registry,
            ToolLoopConfig {
                preamble: None,
                temperature: Some(0.0),
                thinking: None,
                reasoning_effort: None,
                stream: None,
                hook_runner: None,
                allowed_tools: None,
                runtime_context: None,
                max_turns: None,
                repeat_detection_window: Some(DEFAULT_REPEAT_DETECTION_WINDOW),
                repeat_warning_threshold: Some(DEFAULT_REPEAT_WARNING_THRESHOLD),
                preserve_reasoning_content: false,
            },
            &mut observer,
        )
        .await
        .unwrap();

        assert_eq!(turn.final_text, "done");
        assert_eq!(
            observer.begins,
            vec![("call_1".to_string(), "mock_tool".to_string())]
        );
        assert_eq!(
            observer.ends,
            vec![("call_1".to_string(), "mock_tool".to_string(), true)]
        );

        // User(prompt) + Assistant(toolcall) + User(toolresult) + Assistant(text)
        assert_eq!(turn.history.len(), 4);
        assert!(matches!(&turn.history[0], Message::User { .. }));
        assert!(matches!(&turn.history[1], Message::Assistant { .. }));
        assert!(matches!(&turn.history[2], Message::User { .. }));
        assert!(matches!(&turn.history[3], Message::Assistant { .. }));
    }

    #[tokio::test]
    async fn tool_loop_clears_assistant_reasoning_content_by_default() {
        #[derive(Clone)]
        struct ReasoningToolModel {
            seen_followup_reasoning: Arc<Mutex<Vec<Option<String>>>>,
        }

        impl CompletionModel for ReasoningToolModel {
            type Response = ();

            async fn completion(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                let has_tool_result = request.chat_history.iter().any(|msg| match msg {
                    Message::User { content } => content
                        .iter()
                        .any(|item| matches!(item, UserContent::ToolResult(_))),
                    _ => false,
                });

                if has_tool_result {
                    let reasoning = request.chat_history.iter().find_map(|msg| match msg {
                        Message::Assistant {
                            reasoning_content, ..
                        } => reasoning_content.clone(),
                        _ => None,
                    });
                    self.seen_followup_reasoning.lock().unwrap().push(reasoning);

                    return Ok(CompletionResponse {
                        content: vec![AssistantContent::Text(Text {
                            text: "done".to_string(),
                        })],
                        reasoning_content: Some("final reasoning".to_string()),
                        raw_response: (),
                    });
                }

                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "mock_tool".to_string(),
                        function: Function {
                            name: "mock_tool".to_string(),
                            arguments: json!({"value": 1}),
                        },
                    })],
                    reasoning_content: Some("need tool result".to_string()),
                    raw_response: (),
                })
            }
        }

        let seen_followup_reasoning = Arc::new(Mutex::new(Vec::new()));
        let model = ReasoningToolModel {
            seen_followup_reasoning: Arc::clone(&seen_followup_reasoning),
        };
        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        let turn = run_tool_loop_with_history_and_observer(
            &model,
            Vec::new(),
            "hello",
            &registry,
            ToolLoopConfig::default(),
            &mut RecordingObserver::default(),
        )
        .await
        .unwrap();

        assert_eq!(turn.final_text, "done");
        assert_eq!(seen_followup_reasoning.lock().unwrap().as_slice(), &[None]);
        assert!(matches!(
            &turn.history[1],
            Message::Assistant {
                reasoning_content: None,
                ..
            }
        ));
        assert!(matches!(
            &turn.history[3],
            Message::Assistant {
                reasoning_content: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn tool_loop_preserves_assistant_reasoning_content_when_configured() {
        #[derive(Clone)]
        struct ReasoningToolModel {
            seen_followup_reasoning: Arc<Mutex<Vec<Option<String>>>>,
        }

        impl CompletionModel for ReasoningToolModel {
            type Response = ();

            async fn completion(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                let has_tool_result = request.chat_history.iter().any(|msg| match msg {
                    Message::User { content } => content
                        .iter()
                        .any(|item| matches!(item, UserContent::ToolResult(_))),
                    _ => false,
                });

                if has_tool_result {
                    let reasoning = request.chat_history.iter().find_map(|msg| match msg {
                        Message::Assistant {
                            reasoning_content, ..
                        } => reasoning_content.clone(),
                        _ => None,
                    });
                    self.seen_followup_reasoning.lock().unwrap().push(reasoning);

                    return Ok(CompletionResponse {
                        content: vec![AssistantContent::Text(Text {
                            text: "done".to_string(),
                        })],
                        reasoning_content: Some("final reasoning".to_string()),
                        raw_response: (),
                    });
                }

                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_1".to_string(),
                        name: "mock_tool".to_string(),
                        function: Function {
                            name: "mock_tool".to_string(),
                            arguments: json!({"value": 1}),
                        },
                    })],
                    reasoning_content: Some("need tool result".to_string()),
                    raw_response: (),
                })
            }
        }

        let seen_followup_reasoning = Arc::new(Mutex::new(Vec::new()));
        let model = ReasoningToolModel {
            seen_followup_reasoning: Arc::clone(&seen_followup_reasoning),
        };
        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        let turn = run_tool_loop_with_history_and_observer(
            &model,
            Vec::new(),
            "hello",
            &registry,
            ToolLoopConfig {
                preserve_reasoning_content: true,
                ..Default::default()
            },
            &mut RecordingObserver::default(),
        )
        .await
        .unwrap();

        assert_eq!(turn.final_text, "done");
        assert_eq!(
            seen_followup_reasoning.lock().unwrap().as_slice(),
            &[Some("need tool result".to_string())]
        );
        assert!(matches!(
            &turn.history[1],
            Message::Assistant {
                reasoning_content: Some(reasoning),
                ..
            } if reasoning == "need tool result"
        ));
        assert!(matches!(
            &turn.history[3],
            Message::Assistant {
                reasoning_content: Some(reasoning),
                ..
            } if reasoning == "final reasoning"
        ));
    }

    #[tokio::test]
    async fn tool_loop_hook_blocks_tool_call() {
        use crate::internal::ai::hooks::{
            HookRunner,
            config::{HookConfig, HookDefinition},
            event::HookEvent,
        };

        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        // Create a HookRunner with a PreToolUse hook that blocks mock_tool
        let hook_runner = HookRunner::new(
            HookConfig {
                hooks: vec![HookDefinition {
                    event: HookEvent::PreToolUse,
                    matcher: "mock_tool".to_string(),
                    command:
                        r#"exec 0<&-; sleep 0.05; echo "{\"message\":\"tool blocked by test hook\"}"; exit 129"#
                            .to_string(),
                    description: "test blocker".to_string(),
                    timeout_ms: 5000,
                    enabled: true,
                }],
            },
            temp_dir.path().to_path_buf(),
        );

        let mut observer = RecordingObserver::default();
        let turn = run_tool_loop_with_history_and_observer(
            &MockModel,
            Vec::new(),
            "hello",
            &registry,
            ToolLoopConfig {
                hook_runner: Some(Arc::new(hook_runner)),
                ..Default::default()
            },
            &mut observer,
        )
        .await
        .unwrap();

        // The tool call should have been issued (begin) but blocked (end with failure)
        assert_eq!(observer.begins.len(), 1);
        assert_eq!(observer.begins[0].1, "mock_tool");
        assert_eq!(observer.ends.len(), 1);
        // The end should show failure (blocked)
        assert!(
            !observer.ends[0].2,
            "blocked tool call should report as not successful"
        );

        // Model should still produce final text after seeing the block result
        assert_eq!(turn.final_text, "done");

        // History: User(prompt) + Assistant(toolcall) + User(blocked result) + Assistant(text)
        assert_eq!(turn.history.len(), 4);
    }

    #[tokio::test]
    async fn tool_loop_tool_error_is_reported_to_model() {
        /// A handler that always fails.
        struct FailingHandler;

        #[async_trait]
        impl ToolHandler for FailingHandler {
            fn kind(&self) -> ToolKind {
                ToolKind::Function
            }

            async fn handle(
                &self,
                _invocation: ToolInvocation,
            ) -> crate::internal::ai::tools::ToolResult<ToolOutput> {
                Err(crate::internal::ai::tools::ToolError::ExecutionFailed(
                    "something went wrong".to_string(),
                ))
            }

            fn schema(&self) -> ToolSpec {
                ToolSpec::new("failing_tool", "a tool that always fails")
            }
        }

        /// A model that calls failing_tool on first turn, then returns text.
        #[derive(Clone)]
        struct FailToolModel;

        impl CompletionModel for FailToolModel {
            type Response = ();

            async fn completion(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                let has_tool_result = request.chat_history.iter().any(|msg| match msg {
                    Message::User { content } => content
                        .iter()
                        .any(|c| matches!(c, UserContent::ToolResult(_))),
                    _ => false,
                });

                if !has_tool_result {
                    return Ok(CompletionResponse {
                        content: vec![AssistantContent::ToolCall(ToolCall {
                            id: "call_fail".to_string(),
                            name: "failing_tool".to_string(),
                            function: Function {
                                name: "failing_tool".to_string(),
                                arguments: json!({}),
                            },
                        })],
                        reasoning_content: None,
                        raw_response: (),
                    });
                }

                Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: "handled error".to_string(),
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }

        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("failing_tool", Arc::new(FailingHandler));

        let mut observer = RecordingObserver::default();
        let turn = run_tool_loop_with_history_and_observer(
            &FailToolModel,
            Vec::new(),
            "try the tool",
            &registry,
            ToolLoopConfig {
                preamble: None,
                temperature: Some(0.0),
                thinking: None,
                reasoning_effort: None,
                stream: None,
                hook_runner: None,
                allowed_tools: None,
                runtime_context: None,
                max_turns: None,
                repeat_detection_window: Some(DEFAULT_REPEAT_DETECTION_WINDOW),
                repeat_warning_threshold: Some(DEFAULT_REPEAT_WARNING_THRESHOLD),
                preserve_reasoning_content: false,
            },
            &mut observer,
        )
        .await
        .unwrap();

        // Tool call should have been attempted and failed
        assert_eq!(observer.begins.len(), 1);
        assert_eq!(observer.ends.len(), 1);
        assert!(
            !observer.ends[0].2,
            "failed tool should report as not successful"
        );

        // Model should still get the error and produce final text
        assert_eq!(turn.final_text, "handled error");
    }

    #[tokio::test]
    async fn tool_loop_allowed_tools_filters_definitions() {
        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        // With allowed_tools that doesn't include "mock_tool",
        // the model shouldn't see the tool and should return text directly.
        // But our MockModel always issues a tool call first, so the tool call will
        // fail because it's not in the registry's dispatch (it IS in the registry,
        // but allowed_tools only affects the definitions sent to the model).
        // For a proper test, we verify registry_tool_definitions filtering:
        let all_tools = registry_tool_definitions(&registry);
        assert_eq!(all_tools.len(), 1);
        assert_eq!(all_tools[0].name, "mock_tool");

        // Filter with allowed_tools
        let mut filtered = registry_tool_definitions(&registry);
        let allowed = ["nonexistent_tool"];
        filtered.retain(|t| allowed.contains(&t.name.as_str()));
        assert!(filtered.is_empty());

        // Filter with allowed_tools that includes mock_tool
        let mut filtered = registry_tool_definitions(&registry);
        let allowed = ["mock_tool"];
        filtered.retain(|t| allowed.contains(&t.name.as_str()));
        assert_eq!(filtered.len(), 1);
    }

    #[tokio::test]
    async fn tool_loop_allowed_tools_blocks_execution() {
        // MockModel always calls "mock_tool" on first turn.
        // allowed_tools = ["other_tool"] should block "mock_tool" at execution time,
        // returning an error to the model which then produces "done".
        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        let mut observer = RecordingObserver::default();
        let turn = run_tool_loop_with_history_and_observer(
            &MockModel,
            Vec::new(),
            "hello",
            &registry,
            ToolLoopConfig {
                preamble: None,
                temperature: Some(0.0),
                thinking: None,
                reasoning_effort: None,
                stream: None,
                hook_runner: None,
                allowed_tools: Some(vec!["other_tool".to_string()]),
                runtime_context: None,
                max_turns: None,
                repeat_detection_window: Some(DEFAULT_REPEAT_DETECTION_WINDOW),
                repeat_warning_threshold: Some(DEFAULT_REPEAT_WARNING_THRESHOLD),
                preserve_reasoning_content: false,
            },
            &mut observer,
        )
        .await
        .unwrap();

        assert_eq!(turn.final_text, "done");

        // The tool call should have been begun (observer fires before the check)
        assert_eq!(observer.begins.len(), 1);
        assert_eq!(observer.begins[0].1, "mock_tool");

        // The tool call should have ended with failure (blocked)
        assert_eq!(observer.ends.len(), 1);
        assert!(
            !observer.ends[0].2,
            "blocked tool call should report as not successful"
        );
    }

    #[tokio::test]
    async fn tool_loop_stops_when_max_turns_is_reached() {
        #[derive(Clone)]
        struct EndlessToolCallModel;

        impl CompletionModel for EndlessToolCallModel {
            type Response = ();

            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_loop".to_string(),
                        name: "mock_tool".to_string(),
                        function: Function {
                            name: "mock_tool".to_string(),
                            arguments: json!({"value": 1}),
                        },
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }

        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        let err = run_tool_loop_with_history_and_observer(
            &EndlessToolCallModel,
            Vec::new(),
            "loop",
            &registry,
            ToolLoopConfig {
                max_turns: Some(3),
                ..Default::default()
            },
            &mut RecordingObserver::default(),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(err, CompletionError::ResponseError(msg) if msg.contains("maximum turns"))
        );
    }

    #[tokio::test]
    async fn tool_loop_warns_on_repeated_executed_tool_call() {
        #[derive(Clone)]
        struct RepeatingToolModel;

        impl CompletionModel for RepeatingToolModel {
            type Response = ();

            async fn completion(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                let tool_result_count = request
                    .chat_history
                    .iter()
                    .filter_map(|msg| match msg {
                        Message::User { content } => Some(
                            content
                                .iter()
                                .filter(|item| matches!(item, UserContent::ToolResult(_)))
                                .count(),
                        ),
                        _ => None,
                    })
                    .sum::<usize>();

                if tool_result_count >= 3 {
                    return Ok(CompletionResponse {
                        content: vec![AssistantContent::Text(Text {
                            text: "done".to_string(),
                        })],
                        reasoning_content: None,
                        raw_response: (),
                    });
                }

                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: format!("call_{tool_result_count}"),
                        name: "mock_tool".to_string(),
                        function: Function {
                            name: "mock_tool".to_string(),
                            arguments: json!({"b": 2, "a": 1}),
                        },
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }

        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        let turn = run_tool_loop_with_history_and_observer(
            &RepeatingToolModel,
            Vec::new(),
            "repeat",
            &registry,
            ToolLoopConfig::default(),
            &mut RecordingObserver::default(),
        )
        .await
        .unwrap();

        let tool_result_contents = turn
            .history
            .iter()
            .filter_map(|msg| match msg {
                Message::User { content } => content.iter().find_map(|item| match item {
                    UserContent::ToolResult(result) => {
                        result.result["content"].as_str().map(str::to_string)
                    }
                    _ => None,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(tool_result_contents.len(), 3);
        assert!(!tool_result_contents[0].contains("repeated tool call"));
        assert!(!tool_result_contents[1].contains("repeated tool call"));
        assert!(tool_result_contents[2].contains("repeated tool call"));
        assert!(tool_result_contents[2].contains("same arguments 3 times"));
    }

    #[tokio::test]
    async fn tool_loop_stops_on_repeated_blocked_identical_calls() {
        #[derive(Clone)]
        struct BlockedLoopModel;

        impl CompletionModel for BlockedLoopModel {
            type Response = ();

            async fn completion(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
                Ok(CompletionResponse {
                    content: vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_blocked".to_string(),
                        name: "mock_tool".to_string(),
                        function: Function {
                            name: "mock_tool".to_string(),
                            arguments: json!({"value": 7}),
                        },
                    })],
                    reasoning_content: None,
                    raw_response: (),
                })
            }
        }

        #[derive(Default)]
        struct AlwaysBlockPreflightObserver;

        impl ToolLoopObserver for AlwaysBlockPreflightObserver {
            fn on_tool_call_preflight(
                &mut self,
                _call_id: &str,
                _tool_name: &str,
                _arguments: &Value,
            ) -> Result<(), String> {
                Err("blocked by test".to_string())
            }
        }

        let temp_dir = TempDir::new().unwrap();
        let mut registry = ToolRegistry::with_working_dir(temp_dir.path().to_path_buf());
        registry.register("mock_tool", Arc::new(MockHandler));

        let err = run_tool_loop_with_history_and_observer(
            &BlockedLoopModel,
            Vec::new(),
            "loop",
            &registry,
            ToolLoopConfig {
                max_turns: Some(20),
                ..Default::default()
            },
            &mut AlwaysBlockPreflightObserver,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(err, CompletionError::ResponseError(msg) if msg.contains("repeated blocked calls"))
        );
    }
}
