use std::sync::Arc;

use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use dagrs::{
    Action, Content, DefaultNode, Graph, InChannels, Node, NodeTable, OutChannels, Output,
};

use crate::internal::ai::{
    client::CompletionClient,
    completion::CompletionModel,
    node_adapter::ToolLoopAction,
    providers::{
        anthropic::{CLAUDE_3_5_SONNET, Client as AnthropicClient},
        gemini::{Client as GeminiClient, GEMINI_2_5_FLASH},
        openai::{Client as OpenAIClient, GPT_4O_MINI},
    },
    tools::{
        ToolRegistryBuilder,
        handlers::{ApplyPatchHandler, GrepFilesHandler, ListDirHandler, ReadFileHandler},
    },
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum AiProvider {
    Gemini,
    Openai,
    Anthropic,
}

#[derive(Parser, Debug)]
pub struct AiArgs {
    /// The task to execute (e.g. "rename foo to bar in src/main.rs").
    pub prompt: String,

    /// Provider backend.
    #[arg(long, value_enum, default_value_t = AiProvider::Gemini)]
    pub provider: AiProvider,

    /// Model id (provider-specific). If omitted, uses a provider default.
    #[arg(long)]
    pub model: Option<String>,

    /// Maximum model/tool turns before aborting.
    #[arg(long, default_value_t = 8)]
    pub max_steps: usize,

    /// Sampling temperature for model output.
    #[arg(long)]
    pub temperature: Option<f64>,
}

pub async fn execute(args: AiArgs) {
    let working_dir = match std::env::current_dir() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("error: failed to get current working directory: {}", err);
            return;
        }
    };

    let registry = ToolRegistryBuilder::with_working_dir(working_dir.clone())
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .build();

    let preamble = Some(system_preamble(&working_dir));
    let temperature = args.temperature;
    let max_steps = args.max_steps;
    let prompt = args.prompt;

    let output = match args.provider {
        AiProvider::Gemini => {
            let client = match GeminiClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: GEMINI_API_KEY is not set");
                    return;
                }
            };
            let model_name = args.model.unwrap_or_else(|| GEMINI_2_5_FLASH.to_string());
            let model = client.completion_model(&model_name);
            run_graph_blocking(model, registry, preamble, temperature, max_steps, prompt).await
        }
        AiProvider::Openai => {
            let client = match OpenAIClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: OPENAI_API_KEY is not set");
                    return;
                }
            };
            let model_name = args.model.unwrap_or_else(|| GPT_4O_MINI.to_string());
            let model = client.completion_model(&model_name);
            run_graph_blocking(model, registry, preamble, temperature, max_steps, prompt).await
        }
        AiProvider::Anthropic => {
            let client = match AnthropicClient::from_env() {
                Ok(client) => client,
                Err(_) => {
                    eprintln!("error: ANTHROPIC_API_KEY is not set");
                    return;
                }
            };
            let model_name = args.model.unwrap_or_else(|| CLAUDE_3_5_SONNET.to_string());
            let model = client.completion_model(&model_name);
            run_graph_blocking(model, registry, preamble, temperature, max_steps, prompt).await
        }
    };

    match output {
        Ok(Ok(output)) => {
            if !output.trim().is_empty() {
                println!("{}", output.trim());
            }
        }
        Ok(Err(err)) => eprintln!("error: graph execution failed: {}", err),
        Err(err) => eprintln!("error: failed to join graph task: {}", err),
    }
}

async fn run_graph_blocking<M: CompletionModel + 'static>(
    model: M,
    registry: crate::internal::ai::tools::ToolRegistry,
    preamble: Option<String>,
    temperature: Option<f64>,
    max_steps: usize,
    prompt: String,
) -> Result<Result<String, String>, tokio::task::JoinError> {
    // dagrs uses blocking locks while building the graph, so construct and execute it on a blocking thread.
    tokio::task::spawn_blocking(move || {
        let mut node_table = NodeTable::new();

        let input_action = InputGenerator { prompt };
        let a = DefaultNode::with_action("input".to_string(), input_action, &mut node_table);
        let a_id = a.id();

        let tool_action = ToolLoopAction::new(model, registry, preamble, temperature, max_steps);
        let b = DefaultNode::with_action("ai".to_string(), tool_action, &mut node_table);
        let b_id = b.id();

        let mut graph = Graph::new();
        graph.add_node(a);
        graph.add_node(b);
        graph.add_edge(a_id, vec![b_id]);

        graph.start().map_err(|e| format!("{:?}", e))?;

        let outputs = graph.get_results::<String>();
        let output = outputs
            .get(&b_id)
            .cloned()
            .flatten()
            .map(|v| (*v).clone())
            .ok_or_else(|| {
                "AI node produced no string output (node returned empty/non-string output)"
                    .to_string()
            })?;
        Ok::<String, String>(output)
    })
    .await
}

fn system_preamble(working_dir: &std::path::Path) -> String {
    format!(
        "You are a coding agent. Use tools to inspect and modify files. \
All filesystem paths in tool arguments must be absolute and inside this working directory: {}. \
Prefer read_file/list_dir/grep_files before apply_patch. \
When the task is finished, return a short summary of what changed.",
        working_dir.display()
    )
}

struct InputGenerator {
    prompt: String,
}

#[async_trait]
impl Action for InputGenerator {
    async fn run(
        &self,
        _: &mut InChannels,
        out_channels: &mut OutChannels,
        _: Arc<dagrs::EnvVar>,
    ) -> Output {
        let content = Content::new(self.prompt.clone());
        out_channels.broadcast(content.clone()).await;
        Output::Out(Some(content))
    }
}
