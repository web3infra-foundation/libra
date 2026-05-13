//! Prompt package builders for the formal AI runtime phases.
//!
//! Boundary: builders assemble provider-ready text from already-validated contracts;
//! they do not decide workflow state transitions. Runtime contract tests check phase
//! prompts for required context, acceptance gates, and missing-field handling.

use super::contracts::{PromptPackage, WorkflowPhase};

const PLAN_MODE_DEVELOPER_INSTRUCTIONS: &str = concat!(
    "You are in strict plan-first mode. ",
    "Your first responsibility is to generate a structured plan update for the task. ",
    "Do not treat a normal conversational response as sufficient planning output. ",
    "You are replying in a plain terminal interface without Markdown rendering. ",
    "Do not use Markdown headings, bullet markers like '-' or '*', fenced code blocks, tables, or emphasis markers such as '**'. ",
    "Use plain text only. ",
    "Rules: ",
    "First produce a structured step-by-step plan/checklist. ",
    "Do not only explain the plan in prose. ",
    "Use the planning mechanism as the primary planning output whenever possible. ",
    "Do not execute tools, run commands, edit files, or create patches before the user approves the plan. ",
    "After the plan is produced, wait for explicit approval. ",
    "After approval, when files must be created or modified, choose Codex's structured file editing mode that emits fileChange callbacks/events. ",
    "Do not frame this as calling a fileChange tool; instead, choose the editing path that causes the session to produce item.type=fileChange updates. ",
    "Do not use patch-apply style editing or any path that emits apply_patch style events for file edits. ",
    "Prefer the structured edit path that produces fileChange events over commandExecution-based writes. ",
    "CommandExecution may still be used when necessary, but file edits should preferentially use the path that emits fileChange callbacks/events. ",
    "Use commandExecution primarily for inspection, validation, or running the project/tests. ",
    "If you need to change files, prefer the structured edit path that produces fileChange events rather than apply_patch. ",
    "Do not speculate that fileChange is unavailable just because no tool list is shown. ",
    "When the user approves execution and file edits are needed, attempt the structured edit path that should produce fileChange callbacks/events. ",
    "Only say that file writing is blocked if the system explicitly rejects the edit or explicitly reports that the structured fileChange-style path is unavailable. ",
    "Do not ask the user to manually copy code into files unless a real fileChange failure has already happened. ",
    "If the task appears simple, still provide a short structured plan first. ",
    "If you are uncertain, favor planning before acting. ",
    "Keep replies compact, readable, and suitable for direct CLI display. ",
    "The user must see a plan before any execution begins."
);

const PLAN_MODE_BASE_INSTRUCTIONS: &str = concat!(
    "Current mode: strict structured planning first. ",
    "Produce a structured plan before execution, prefer the planning system over prose-only planning, ",
    "wait for user approval before taking action, ",
    "and when modifying files choose the structured editing path that emits fileChange callbacks/events. ",
    "Do not use apply_patch-style editing. ",
    "Prefer the fileChange-emitting path over commandExecution-based file writes. ",
    "Do not claim that the fileChange-style path is unavailable unless the system explicitly reports that failure. ",
    "Reply in plain text without Markdown."
);

#[derive(Clone, Debug)]
pub struct IntentPromptBuilder {
    provider: String,
    model: String,
    principal: String,
    request: Option<String>,
}

impl IntentPromptBuilder {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            principal: "libra-runtime".to_string(),
            request: None,
        }
    }

    pub fn principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = principal.into();
        self
    }

    pub fn request(mut self, request: impl Into<String>) -> Self {
        self.request = Some(request.into());
        self
    }

    pub fn build(self) -> PromptPackage {
        PromptPackage {
            phase: WorkflowPhase::Intent,
            provider: self.provider,
            model: self.model,
            preamble: format!(
                "Principal: {}. Refine the user request into a reviewable Libra IntentSpec. Use readonly analysis only.",
                self.principal
            ),
            messages: self.request.into_iter().collect(),
            readonly_tools: vec!["read".to_string(), "search".to_string()],
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlanningPromptBuilder {
    provider: String,
    model: String,
    principal: String,
    intent_summary: Option<String>,
}

impl PlanningPromptBuilder {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            principal: "libra-runtime".to_string(),
            intent_summary: None,
        }
    }

    pub fn principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = principal.into();
        self
    }

    pub fn intent_summary(mut self, intent_summary: impl Into<String>) -> Self {
        self.intent_summary = Some(intent_summary.into());
        self
    }

    pub fn build(self) -> PromptPackage {
        let mut messages = Vec::new();
        if let Some(intent_summary) = self.intent_summary {
            messages.push(intent_summary);
        }
        PromptPackage {
            phase: WorkflowPhase::Planning,
            provider: self.provider,
            model: self.model,
            preamble: format!(
                "Principal: {}. Produce exactly two plan heads: execution and test. Use readonly analysis only.",
                self.principal
            ),
            messages,
            readonly_tools: vec!["read".to_string(), "search".to_string()],
        }
    }

    pub fn codex_plan_mode_developer_instructions() -> &'static str {
        PLAN_MODE_DEVELOPER_INSTRUCTIONS
    }

    pub fn codex_plan_mode_base_instructions() -> &'static str {
        PLAN_MODE_BASE_INSTRUCTIONS
    }
}

#[derive(Clone, Debug)]
pub struct TaskPromptBuilder {
    provider: String,
    model: String,
    principal: String,
    task_title: Option<String>,
    task_objective: Option<String>,
}

impl TaskPromptBuilder {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            principal: "libra-runtime".to_string(),
            task_title: None,
            task_objective: None,
        }
    }

    pub fn principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = principal.into();
        self
    }

    pub fn task(mut self, title: impl Into<String>, objective: impl Into<String>) -> Self {
        self.task_title = Some(title.into());
        self.task_objective = Some(objective.into());
        self
    }

    pub fn build(self) -> PromptPackage {
        let mut messages = Vec::new();
        if let Some(title) = self.task_title {
            messages.push(format!("Task: {title}"));
        }
        if let Some(objective) = self.task_objective {
            messages.push(format!("Objective: {objective}"));
        }
        PromptPackage {
            phase: WorkflowPhase::Execution,
            provider: self.provider,
            model: self.model,
            preamble: format!(
                "Principal: {}. Execute one scheduler-assigned task attempt and return structured evidence.",
                self.principal
            ),
            messages,
            readonly_tools: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_builder_keeps_codex_plan_mode_text_stable() {
        assert!(
            PlanningPromptBuilder::codex_plan_mode_developer_instructions()
                .contains("strict plan-first mode")
        );
        assert!(
            PlanningPromptBuilder::codex_plan_mode_base_instructions()
                .contains("structured planning first")
        );
    }

    #[test]
    fn builders_emit_phase_specific_prompt_packages() {
        let intent = IntentPromptBuilder::new("mock", "m")
            .request("implement feature")
            .build();
        assert_eq!(intent.phase, WorkflowPhase::Intent);
        assert_eq!(intent.messages, ["implement feature"]);

        let planning = PlanningPromptBuilder::new("mock", "m")
            .intent_summary("intent confirmed")
            .build();
        assert_eq!(planning.phase, WorkflowPhase::Planning);
        assert!(planning.preamble.contains("execution and test"));

        let task = TaskPromptBuilder::new("mock", "m")
            .task("write code", "change one file")
            .build();
        assert_eq!(task.phase, WorkflowPhase::Execution);
        assert_eq!(task.messages.len(), 2);
    }
}
