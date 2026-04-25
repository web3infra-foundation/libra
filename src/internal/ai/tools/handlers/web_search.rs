//! Handler for the web_search tool.
//!
//! The tool intentionally returns compact search result metadata rather than
//! fetching arbitrary pages. Page retrieval can be added as a separate tool with
//! its own trust and output-size controls.

use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use url::Url;

use super::parse_arguments;
use crate::internal::ai::tools::{
    context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload, WebSearchArgs},
    error::{ToolError, ToolResult},
    registry::ToolHandler,
    spec::ToolSpec,
};

const DUCKDUCKGO_HTML_SEARCH_URL: &str = "https://html.duckduckgo.com/html/";
const WEB_SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_WEB_SEARCH_RESULTS: usize = 10;
const MAX_SNIPPET_CHARS: usize = 320;

/// Handler for public web search.
pub struct WebSearchHandler;

#[derive(Debug, Clone, PartialEq, Eq)]
struct WebSearchResult {
    title: String,
    url: String,
    snippet: Option<String>,
}

#[derive(Debug)]
struct RawSearchResult {
    start: usize,
    end: usize,
    title_html: String,
    href: String,
}

#[async_trait]
impl ToolHandler for WebSearchHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        ensure_network_allowed(&invocation)?;

        let arguments = match invocation.payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(ToolError::IncompatiblePayload(
                    "web_search handler only accepts Function payloads".to_string(),
                ));
            }
        };

        let args: WebSearchArgs = parse_arguments(&arguments)?;
        let query = args.query.trim();
        if query.is_empty() {
            return Err(ToolError::InvalidArguments(
                "web_search query must not be empty".to_string(),
            ));
        }

        let limit = args.limit.clamp(1, MAX_WEB_SEARCH_RESULTS);
        let html = fetch_duckduckgo_html(query).await?;
        let results = parse_duckduckgo_results(&html, limit)?;

        Ok(ToolOutput::success(format_web_search_results(
            query, &results,
        )))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::web_search()
    }
}

fn ensure_network_allowed(invocation: &ToolInvocation) -> ToolResult<()> {
    let Some(runtime_context) = invocation.runtime_context.as_ref() else {
        return Ok(());
    };
    let Some(sandbox) = runtime_context.sandbox.as_ref() else {
        return Ok(());
    };

    if sandbox.policy.has_full_network_access() {
        Ok(())
    } else {
        Err(ToolError::ExecutionFailed(
            "web_search requires network access, but the current tool runtime has network access disabled. Enable Network: Allow for the plan or start the TUI with network access allowed."
                .to_string(),
        ))
    }
}

async fn fetch_duckduckgo_html(query: &str) -> ToolResult<String> {
    let mut url = Url::parse(DUCKDUCKGO_HTML_SEARCH_URL)
        .map_err(|error| ToolError::ExecutionFailed(format!("invalid web search URL: {error}")))?;
    url.query_pairs_mut().append_pair("q", query);

    let client = reqwest::Client::builder()
        .timeout(WEB_SEARCH_TIMEOUT)
        .user_agent("libra-code/0.1 (+https://github.com/web3infra-foundation/mega)")
        .build()
        .map_err(|error| {
            ToolError::ExecutionFailed(format!("failed to initialize web search client: {error}"))
        })?;

    let response = client.get(url).send().await.map_err(|error| {
        ToolError::ExecutionFailed(format!("failed to run web search request: {error}"))
    })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        ToolError::ExecutionFailed(format!("failed to read web search response: {error}"))
    })?;

    if !status.is_success() {
        return Err(ToolError::ExecutionFailed(format!(
            "web search provider returned HTTP {}: {}",
            status.as_u16(),
            body.lines().next().unwrap_or_default()
        )));
    }

    Ok(body)
}

fn parse_duckduckgo_results(html: &str, limit: usize) -> ToolResult<Vec<WebSearchResult>> {
    let link_re = Regex::new(
        r#"(?is)<a\b[^>]*class="[^"]*\bresult__a\b[^"]*"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#,
    )
    .map_err(|error| {
        ToolError::ExecutionFailed(format!("failed to compile web search link parser: {error}"))
    })?;
    let snippet_re =
        Regex::new(r#"(?is)<a\b[^>]*class="[^"]*\bresult__snippet\b[^"]*"[^>]*>(.*?)</a>"#)
            .map_err(|error| {
                ToolError::ExecutionFailed(format!(
                    "failed to compile web search snippet parser: {error}"
                ))
            })?;

    let mut raw_results = Vec::new();
    for captures in link_re.captures_iter(html) {
        let Some(full_match) = captures.get(0) else {
            continue;
        };
        let Some(href) = captures.get(1).map(|m| m.as_str().to_string()) else {
            continue;
        };
        let Some(title_html) = captures.get(2).map(|m| m.as_str().to_string()) else {
            continue;
        };
        raw_results.push(RawSearchResult {
            start: full_match.start(),
            end: full_match.end(),
            title_html,
            href,
        });
    }

    let mut results = Vec::new();
    for (idx, raw) in raw_results.iter().enumerate() {
        if results.len() >= limit {
            break;
        }

        let next_start = raw_results
            .get(idx + 1)
            .map(|next| next.start)
            .unwrap_or(html.len());
        let block = html.get(raw.end..next_start).unwrap_or_default();
        let snippet = snippet_re
            .captures(block)
            .and_then(|captures| captures.get(1))
            .map(|value| clean_html_text(value.as_str()))
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(&value, MAX_SNIPPET_CHARS));

        let title = clean_html_text(&raw.title_html);
        let url = decode_duckduckgo_result_url(&raw.href);
        if title.is_empty() || url.is_empty() {
            continue;
        }

        results.push(WebSearchResult {
            title,
            url,
            snippet,
        });
    }

    Ok(results)
}

fn decode_duckduckgo_result_url(raw: &str) -> String {
    let normalized = if raw.starts_with("//") {
        format!("https:{raw}")
    } else if raw.starts_with('/') {
        format!("https://duckduckgo.com{raw}")
    } else {
        raw.to_string()
    };
    let normalized = normalized.replace("&amp;", "&");

    if let Ok(url) = Url::parse(&normalized)
        && url
            .domain()
            .is_some_and(|domain| domain.ends_with("duckduckgo.com"))
        && url.path().starts_with("/l/")
        && let Some((_, target)) = url.query_pairs().find(|(key, _)| key == "uddg")
    {
        return target.into_owned();
    }

    normalized
}

fn clean_html_text(raw: &str) -> String {
    let without_tags = strip_html_tags(raw);
    collapse_whitespace(&decode_html_entities(&without_tags))
}

fn strip_html_tags(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output
}

fn decode_html_entities(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut rest = raw;

    while let Some(pos) = rest.find('&') {
        output.push_str(&rest[..pos]);
        let after_amp = &rest[pos + 1..];
        if let Some(end) = after_amp.find(';') {
            let entity = &after_amp[..end];
            if let Some(decoded) = decode_html_entity(entity) {
                output.push_str(&decoded);
                rest = &after_amp[end + 1..];
                continue;
            }
        }
        output.push('&');
        rest = after_amp;
    }

    output.push_str(rest);
    output
}

fn decode_html_entity(entity: &str) -> Option<String> {
    match entity {
        "amp" => Some("&".to_string()),
        "lt" => Some("<".to_string()),
        "gt" => Some(">".to_string()),
        "quot" => Some("\"".to_string()),
        "apos" => Some("'".to_string()),
        "nbsp" => Some(" ".to_string()),
        _ => decode_numeric_entity(entity),
    }
}

fn decode_numeric_entity(entity: &str) -> Option<String> {
    let value = if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else if let Some(decimal) = entity.strip_prefix('#') {
        decimal.parse::<u32>().ok()?
    } else {
        return None;
    };
    char::from_u32(value).map(|ch| ch.to_string())
}

fn collapse_whitespace(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(raw: &str, limit: usize) -> String {
    if raw.chars().count() <= limit {
        return raw.to_string();
    }
    let mut truncated = raw
        .chars()
        .take(limit.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn format_web_search_results(query: &str, results: &[WebSearchResult]) -> String {
    if results.is_empty() {
        return format!("No web search results found for \"{query}\".");
    }

    let mut lines = vec![format!("Web search results for \"{query}\":")];
    for (idx, result) in results.iter().enumerate() {
        lines.push(format!("{}. {}", idx + 1, result.title));
        lines.push(format!("   URL: {}", result.url));
        if let Some(snippet) = result.snippet.as_deref()
            && !snippet.is_empty()
        {
            lines.push(format!("   Snippet: {snippet}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::internal::ai::sandbox::{
        SandboxPermissions, SandboxPolicy, ToolRuntimeContext, ToolSandboxContext,
    };

    #[test]
    fn parses_duckduckgo_html_results() {
        let html = r#"
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fblog.rust-lang.org%2F2025%2F02%2F20%2FRust-1.85.0%2F&amp;rut=abc">Announcing <b>Rust</b> 1.85.0 and Rust 2024</a>
            <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fblog.rust-lang.org%2F2025%2F02%2F20%2FRust-1.85.0%2F&amp;rut=abc">This stabilizes the <b>2024</b> edition as well.</a>
            <a rel="nofollow" class="result__a" href="https://example.com/plain">Plain &amp; Simple</a>
            <a class="result__snippet" href="https://example.com/plain">A second result &#x27;snippet&#x27;.</a>
        "#;

        let results = parse_duckduckgo_results(html, 5).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].url,
            "https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/"
        );
        assert_eq!(results[0].title, "Announcing Rust 1.85.0 and Rust 2024");
        assert_eq!(
            results[0].snippet.as_deref(),
            Some("This stabilizes the 2024 edition as well.")
        );
        assert_eq!(results[1].title, "Plain & Simple");
        assert_eq!(
            results[1].snippet.as_deref(),
            Some("A second result 'snippet'.")
        );
    }

    #[test]
    fn web_search_requires_network_enabled_runtime() {
        let invocation = ToolInvocation::new(
            "call-1",
            "web_search",
            ToolPayload::Function {
                arguments: serde_json::json!({"query": "rust 2024"}).to_string(),
            },
            PathBuf::from("/tmp"),
        )
        .with_runtime_context(ToolRuntimeContext {
            sandbox: Some(ToolSandboxContext {
                policy: SandboxPolicy::WorkspaceWrite {
                    writable_roots: vec![PathBuf::from("/tmp")],
                    network_access: false,
                    exclude_tmpdir_env_var: false,
                    exclude_slash_tmp: false,
                },
                permissions: SandboxPermissions::UseDefault,
            }),
            ..ToolRuntimeContext::default()
        });

        let error = ensure_network_allowed(&invocation).unwrap_err();

        assert!(error.to_string().contains("requires network access"));
    }
}
