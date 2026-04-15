//! Shared runtime contracts for the `libra code` workflow.
//!
//! Phase 0 keeps this module contract-only so existing provider paths can adapt
//! to one stable surface before scheduler and provider cutover starts.

pub mod contracts;
pub mod environment;
pub mod phase3;
pub mod phase4;
pub mod prompt_builders;

pub use contracts::{PromptPackage, WorkflowPhase};
pub use phase3::{
    ArtifactLedger, ValidationOutcome, ValidationReport, ValidationReportStore, ValidationStage,
    ValidationStageResult, ValidatorEngine,
};
pub use phase4::{
    DecisionPolicy, DecisionProposal, DecisionProposalRoute, DecisionProposalStore,
    RiskScoreBreakdown, aggregate_risk_score, build_decision_proposal,
};
pub use prompt_builders::{IntentPromptBuilder, PlanningPromptBuilder, TaskPromptBuilder};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub principal: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            principal: "libra-runtime".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Runtime {
    config: RuntimeConfig,
}

impl Runtime {
    pub fn new(config: RuntimeConfig) -> Self {
        Self { config }
    }

    pub fn principal(&self) -> &str {
        &self.config.principal
    }

    pub fn intent_prompt_builder(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> IntentPromptBuilder {
        IntentPromptBuilder::new(provider, model).principal(self.principal())
    }

    pub fn planning_prompt_builder(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> PlanningPromptBuilder {
        PlanningPromptBuilder::new(provider, model).principal(self.principal())
    }

    pub fn task_prompt_builder(
        &self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> TaskPromptBuilder {
        TaskPromptBuilder::new(provider, model).principal(self.principal())
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new(RuntimeConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_exposes_narrow_prompt_builder_entrypoints() {
        let runtime = Runtime::new(RuntimeConfig {
            principal: "tester".into(),
        });
        let package = runtime
            .intent_prompt_builder("mock", "model")
            .request("make tests pass")
            .build();

        assert_eq!(runtime.principal(), "tester");
        assert_eq!(package.phase, WorkflowPhase::Intent);
        assert_eq!(package.provider, "mock");
        assert!(package.preamble.contains("IntentSpec"));
    }
}
