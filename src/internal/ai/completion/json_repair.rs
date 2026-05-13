//! Provider-neutral JSON repair helpers for weak-model tool-call arguments.
//!
//! Boundary: this module is deliberately deterministic and local. It repairs
//! common JSON-shaped model output before provider adapters decide whether to
//! surface a malformed tool call. It does not validate tool schemas or execute
//! repaired arguments.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRepairOutcome {
    pub value: Value,
    pub repaired_source: String,
    pub repaired: bool,
    pub fixes: Vec<JsonRepairFix>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRepairFix {
    pub kind: JsonRepairFixKind,
}

impl JsonRepairFix {
    fn new(kind: JsonRepairFixKind) -> Self {
        Self { kind }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonRepairFixKind {
    ExtractedJsonCandidate,
    StrippedCodeFence,
    NormalizedSmartQuotes,
    StrippedComments,
    NormalizedPythonLiterals,
    ConvertedSingleQuotedStrings,
    QuotedObjectKeys,
    RemovedTrailingCommas,
    WrappedTopLevelObjectFields,
    BalancedDelimiters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonRepairErrorKind {
    EmptyInput,
    NoJsonCandidate,
    ParseFailed,
}

impl JsonRepairErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmptyInput => "empty_input",
            Self::NoJsonCandidate => "no_json_candidate",
            Self::ParseFailed => "parse_failed",
        }
    }
}

impl fmt::Display for JsonRepairErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{kind}: {message}")]
pub struct JsonRepairError {
    pub kind: JsonRepairErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_error: Option<String>,
}

impl JsonRepairError {
    fn new(
        kind: JsonRepairErrorKind,
        message: impl Into<String>,
        parse_error: Option<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            parse_error,
        }
    }
}

#[derive(Clone, Debug)]
struct JsonRepairCandidate {
    source: String,
    fixes: Vec<JsonRepairFix>,
}

/// Parse a JSON value, repairing common weak-model formatting defects.
pub fn parse_json_repaired(input: &str) -> Result<JsonRepairOutcome, JsonRepairError> {
    let trimmed = trim_json_input(input);
    if trimmed.is_empty() {
        return Err(JsonRepairError::new(
            JsonRepairErrorKind::EmptyInput,
            "JSON repair input is empty",
            None,
        ));
    }

    let candidates = json_repair_candidates(trimmed);
    if candidates.is_empty() {
        return Err(JsonRepairError::new(
            JsonRepairErrorKind::NoJsonCandidate,
            "input does not contain a JSON object, array, or top-level field list",
            None,
        ));
    }

    let mut last_parse_error = None;
    for candidate in candidates {
        if let Ok(value) = serde_json::from_str::<Value>(&candidate.source) {
            let repaired = candidate.source != trimmed || !candidate.fixes.is_empty();
            return Ok(JsonRepairOutcome {
                value,
                repaired_source: candidate.source,
                repaired,
                fixes: candidate.fixes,
            });
        }

        let repaired = repair_json_candidate(&candidate);
        match serde_json::from_str::<Value>(&repaired.source) {
            Ok(value) => {
                let repaired_flag = repaired.source != trimmed || !repaired.fixes.is_empty();
                return Ok(JsonRepairOutcome {
                    value,
                    repaired_source: repaired.source,
                    repaired: repaired_flag,
                    fixes: repaired.fixes,
                });
            }
            Err(error) => {
                last_parse_error = Some(error.to_string());
            }
        }
    }

    Err(JsonRepairError::new(
        JsonRepairErrorKind::ParseFailed,
        "failed to parse JSON after deterministic repair attempts",
        last_parse_error,
    ))
}

/// Parse provider tool-call arguments, repairing common malformed JSON before
/// falling back to the raw string payload.
pub fn parse_tool_call_arguments_with_repair(
    provider: &str,
    tool_name: &str,
    arguments: &str,
) -> Value {
    match serde_json::from_str::<Value>(arguments) {
        Ok(value) => value,
        Err(original_error) => match parse_json_repaired(arguments) {
            Ok(outcome) => {
                tracing::warn!(
                    provider,
                    tool_name,
                    fixes = ?outcome.fixes,
                    original_error = %original_error,
                    original = %truncate_for_log(arguments, 512),
                    repaired = %truncate_for_log(&outcome.repaired_source, 512),
                    "repaired malformed provider tool-call arguments"
                );
                outcome.value
            }
            Err(error) => {
                tracing::warn!(
                    provider,
                    tool_name,
                    error_kind = %error.kind,
                    parse_error = ?error.parse_error,
                    original_error = %original_error,
                    original = %truncate_for_log(arguments, 512),
                    "failed to repair provider tool-call arguments; using raw string"
                );
                Value::String(arguments.to_string())
            }
        },
    }
}

fn truncate_for_log(input: &str, max_chars: usize) -> String {
    let mut output = String::new();
    let mut chars = input.chars();
    for ch in chars.by_ref().take(max_chars) {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            other => output.push(other),
        }
    }
    if chars.next().is_some() {
        output.push_str("...");
    }
    output
}

fn trim_json_input(input: &str) -> &str {
    input.trim_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}')
}

fn json_repair_candidates(trimmed: &str) -> Vec<JsonRepairCandidate> {
    let mut candidates = Vec::new();
    push_candidate(&mut candidates, trimmed.to_string(), Vec::new());

    if let Some(fenced) = strip_code_fence(trimmed) {
        push_candidate(
            &mut candidates,
            fenced,
            vec![JsonRepairFix::new(JsonRepairFixKind::StrippedCodeFence)],
        );
    }

    let initial_candidates = candidates.clone();
    for candidate in initial_candidates {
        if let Some(extracted) = extract_json_candidate(&candidate.source) {
            let mut fixes = candidate.fixes;
            fixes.push(JsonRepairFix::new(
                JsonRepairFixKind::ExtractedJsonCandidate,
            ));
            push_candidate(&mut candidates, extracted, fixes);
        }
    }

    if candidates
        .iter()
        .all(|candidate| !looks_like_jsonish(&candidate.source))
    {
        return Vec::new();
    }

    candidates
}

fn push_candidate(
    candidates: &mut Vec<JsonRepairCandidate>,
    source: String,
    fixes: Vec<JsonRepairFix>,
) {
    let source = trim_json_input(&source).to_string();
    if source.is_empty() {
        return;
    }
    if candidates
        .iter()
        .any(|candidate| candidate.source == source)
    {
        return;
    }
    candidates.push(JsonRepairCandidate { source, fixes });
}

fn strip_code_fence(input: &str) -> Option<String> {
    let start = input.find("```")?;
    let after_ticks = &input[start + 3..];
    let content_start = after_ticks.find('\n').map_or(0, |index| index + 1);
    let content = &after_ticks[content_start..];
    let end = content.find("```").unwrap_or(content.len());
    Some(content[..end].to_string())
}

fn extract_json_candidate(input: &str) -> Option<String> {
    let start = input
        .char_indices()
        .find_map(|(index, ch)| matches!(ch, '{' | '[').then_some((index, ch)))?;
    let close = matching_close(start.1);
    let mut stack = vec![close];
    let mut in_string = false;
    let mut escaped = false;
    let mut found_close = None;

    for (relative_index, ch) in input[start.0 + start.1.len_utf8()..].char_indices() {
        let absolute_index = start.0 + start.1.len_utf8() + relative_index;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' | '[' => stack.push(matching_close(ch)),
            '}' | ']' if stack.last().copied() == Some(ch) => {
                stack.pop();
                if stack.is_empty() {
                    found_close = Some(absolute_index + ch.len_utf8());
                    break;
                }
            }
            '}' | ']' => {}
            _ => {}
        }
    }

    let end = found_close.unwrap_or(input.len());
    Some(input[start.0..end].to_string())
}

fn matching_close(open: char) -> char {
    match open {
        '{' => '}',
        '[' => ']',
        _ => open,
    }
}

fn repair_json_candidate(candidate: &JsonRepairCandidate) -> JsonRepairCandidate {
    let mut source = candidate.source.clone();
    let mut fixes = candidate.fixes.clone();

    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::NormalizedSmartQuotes,
        normalize_smart_quotes,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::StrippedComments,
        strip_json_comments,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::ConvertedSingleQuotedStrings,
        convert_single_quoted_strings,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::NormalizedPythonLiterals,
        normalize_python_literals,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::QuotedObjectKeys,
        quote_unquoted_object_keys,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::RemovedTrailingCommas,
        remove_trailing_commas,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::WrappedTopLevelObjectFields,
        wrap_top_level_object_fields,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::QuotedObjectKeys,
        quote_unquoted_object_keys,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::BalancedDelimiters,
        balance_delimiters,
    );
    apply_repair(
        &mut source,
        &mut fixes,
        JsonRepairFixKind::RemovedTrailingCommas,
        remove_trailing_commas,
    );

    JsonRepairCandidate { source, fixes }
}

fn apply_repair(
    source: &mut String,
    fixes: &mut Vec<JsonRepairFix>,
    kind: JsonRepairFixKind,
    repair: fn(&str) -> Option<String>,
) {
    let Some(repaired) = repair(source) else {
        return;
    };
    if repaired == *source {
        return;
    }
    *source = repaired;
    if !fixes.iter().any(|fix| fix.kind == kind) {
        fixes.push(JsonRepairFix::new(kind));
    }
}

fn normalize_smart_quotes(input: &str) -> Option<String> {
    if !input
        .chars()
        .any(|ch| matches!(ch, '“' | '”' | '„' | '‟' | '‘' | '’' | '‚' | '‛'))
    {
        return None;
    }
    let normalized = input
        .chars()
        .map(|ch| match ch {
            '“' | '”' | '„' | '‟' => '"',
            '‘' | '’' | '‚' | '‛' => '\'',
            other => other,
        })
        .collect();
    Some(normalized)
}

fn strip_json_comments(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut changed = false;

    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }

        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    changed = true;
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            output.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    changed = true;
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        previous = next;
                    }
                }
                _ => output.push(ch),
            }
            continue;
        }

        output.push(ch);
    }

    changed.then_some(output)
}

fn convert_single_quoted_strings(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_double = false;
    let mut escaped = false;
    let mut changed = false;

    while let Some(ch) = chars.next() {
        if in_double {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
            continue;
        }

        if ch == '"' {
            in_double = true;
            output.push(ch);
            continue;
        }

        if ch == '\'' {
            changed = true;
            output.push('"');
            let mut single_escaped = false;
            for next in chars.by_ref() {
                if single_escaped {
                    match next {
                        '\'' => output.push('\''),
                        '"' => output.push_str("\\\""),
                        other => {
                            output.push('\\');
                            output.push(other);
                        }
                    }
                    single_escaped = false;
                    continue;
                }
                match next {
                    '\\' => single_escaped = true,
                    '\'' => {
                        output.push('"');
                        break;
                    }
                    '"' => output.push_str("\\\""),
                    other => output.push(other),
                }
            }
            continue;
        }

        output.push(ch);
    }

    changed.then_some(output)
}

fn normalize_python_literals(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len());
    let chars = input.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut changed = false;

    while index < chars.len() {
        let (byte_index, ch) = chars[index];
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if ch == '"' {
            in_string = true;
            output.push(ch);
            index += 1;
            continue;
        }

        let remaining = &input[byte_index..];
        if token_at(remaining, "True") {
            output.push_str("true");
            index += "True".chars().count();
            changed = true;
        } else if token_at(remaining, "False") {
            output.push_str("false");
            index += "False".chars().count();
            changed = true;
        } else if token_at(remaining, "None") {
            output.push_str("null");
            index += "None".chars().count();
            changed = true;
        } else if token_at(remaining, "undefined") {
            output.push_str("null");
            index += "undefined".chars().count();
            changed = true;
        } else {
            output.push(ch);
            index += 1;
        }
    }

    changed.then_some(output)
}

fn token_at(remaining: &str, token: &str) -> bool {
    remaining.starts_with(token)
        && remaining[token.len()..]
            .chars()
            .next()
            .is_none_or(|ch| !is_identifier_continue(ch))
}

fn quote_unquoted_object_keys(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len() + 8);
    let chars = input.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut previous_significant = None;
    let mut changed = false;

    while index < chars.len() {
        let (_, ch) = chars[index];
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if ch == '"' {
            in_string = true;
            output.push(ch);
            previous_significant = Some(ch);
            index += 1;
            continue;
        }

        if matches!(previous_significant, Some('{') | Some(',')) && is_unquoted_key_start(ch) {
            let key_start = index;
            let mut key_end = index + 1;
            while key_end < chars.len() && is_unquoted_key_continue(chars[key_end].1) {
                key_end += 1;
            }
            let mut colon_index = key_end;
            while colon_index < chars.len() && chars[colon_index].1.is_whitespace() {
                colon_index += 1;
            }
            if colon_index < chars.len() && chars[colon_index].1 == ':' {
                let key_byte_start = chars[key_start].0;
                let key_byte_end = if key_end < chars.len() {
                    chars[key_end].0
                } else {
                    input.len()
                };
                output.push('"');
                output.push_str(&input[key_byte_start..key_byte_end]);
                output.push('"');
                for (_, whitespace) in chars.iter().take(colon_index).skip(key_end) {
                    output.push(*whitespace);
                }
                output.push(':');
                previous_significant = Some(':');
                index = colon_index + 1;
                changed = true;
                continue;
            }
        }

        output.push(ch);
        if !ch.is_whitespace() {
            previous_significant = Some(ch);
        }
        index += 1;
    }

    changed.then_some(output)
}

fn remove_trailing_commas(input: &str) -> Option<String> {
    let chars = input.char_indices().collect::<Vec<_>>();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut changed = false;

    while index < chars.len() {
        let (_, ch) = chars[index];
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if ch == '"' {
            in_string = true;
            output.push(ch);
            index += 1;
            continue;
        }

        if ch == ',' {
            let mut lookahead = index + 1;
            while lookahead < chars.len() && chars[lookahead].1.is_whitespace() {
                lookahead += 1;
            }
            if lookahead >= chars.len() || matches!(chars[lookahead].1, '}' | ']') {
                changed = true;
                index += 1;
                continue;
            }
        }

        output.push(ch);
        index += 1;
    }

    changed.then_some(output)
}

fn wrap_top_level_object_fields(input: &str) -> Option<String> {
    let trimmed = trim_json_input(input);
    if trimmed.starts_with('{') || trimmed.starts_with('[') || !top_level_contains_colon(trimmed) {
        return None;
    }
    Some(format!("{{{trimmed}}}"))
}

fn balance_delimiters(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len() + 4);
    let mut stack = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut changed = false;

    for ch in input.chars() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                output.push(ch);
            }
            '{' => {
                stack.push('}');
                output.push(ch);
            }
            '[' => {
                stack.push(']');
                output.push(ch);
            }
            '}' | ']' => {
                if stack.last().copied() == Some(ch) {
                    stack.pop();
                    output.push(ch);
                } else {
                    changed = true;
                }
            }
            _ => output.push(ch),
        }
    }

    if in_string {
        output.push('"');
        changed = true;
    }
    while let Some(close) = stack.pop() {
        output.push(close);
        changed = true;
    }

    changed.then_some(output)
}

fn looks_like_jsonish(input: &str) -> bool {
    let trimmed = trim_json_input(input);
    trimmed.contains('{')
        || trimmed.contains('[')
        || trimmed.contains("```")
        || top_level_contains_colon(trimmed)
}

fn top_level_contains_colon(input: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;

    for ch in input.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' | '[' => depth = depth.saturating_add(1),
            '}' | ']' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => return true,
            _ => {}
        }
    }

    false
}

fn is_unquoted_key_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn is_unquoted_key_continue(ch: char) -> bool {
    ch == '_' || ch == '$' || ch == '-' || ch.is_ascii_alphanumeric()
}

fn is_identifier_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}
