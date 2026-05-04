//! Source Pool abstractions for external and local capability providers.
//!
//! CEX-14 introduces a source layer above individual tools. A source owns a
//! manifest, trust tier, and session-scoped execution view; tool handlers are
//! generated from that view so legacy MCP tools and future source-prefixed
//! tools use the same validation and audit path.

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Mutex},
    time::Instant,
};

use async_trait::async_trait;

use crate::internal::ai::tools::{
    context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload},
    error::{ToolError, ToolResult},
    registry::ToolHandler,
    spec::ToolSpec,
};

pub mod mcp;

pub use mcp::{BUILTIN_MCP_SOURCE_SLUG, McpSource};

/// Source category. This stays intentionally small for Step 1.10 Phase A.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Mcp,
    RestApi,
    LocalDocs,
}

/// Source trust tier used to decide default enablement and permission review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustTier {
    Builtin,
    Project,
    User,
    ThirdParty,
    Untrusted,
}

impl TrustTier {
    fn requires_explicit_enablement(self) -> bool {
        matches!(self, Self::ThirdParty | Self::Untrusted)
    }
}

/// Coarse source access declaration for the capability manifest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceAccess {
    #[default]
    None,
    WorkspaceRead,
    WorkspaceWrite,
    Workspace,
    Network,
}

/// Credential access declaration for a source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CredentialAccess {
    pub required: bool,
    pub refs: Vec<String>,
}

impl CredentialAccess {
    pub fn none() -> Self {
        Self::default()
    }
}

/// How a source became enabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceEnablement {
    Disabled,
    Builtin,
    ProjectConfig,
    UserConfig,
    SessionExplicit,
}

impl SourceEnablement {
    fn is_enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    fn is_explicit(self) -> bool {
        matches!(
            self,
            Self::ProjectConfig | Self::UserConfig | Self::SessionExplicit
        )
    }

    fn default_for_trust_tier(trust_tier: TrustTier) -> Self {
        match trust_tier {
            TrustTier::Builtin => Self::Builtin,
            TrustTier::Project => Self::ProjectConfig,
            TrustTier::User => Self::UserConfig,
            TrustTier::ThirdParty | TrustTier::Untrusted => Self::Disabled,
        }
    }
}

/// Tool-level declaration inside a [`CapabilityManifest`].
#[derive(Clone, Debug)]
pub struct SourceToolCapability {
    pub name: String,
    pub spec: ToolSpec,
    pub mutating: bool,
    pub requires_network: bool,
    pub approval_scope: Option<String>,
    pub credential_ref: Option<String>,
    pub cost_estimate_micros: Option<u64>,
}

impl SourceToolCapability {
    pub fn new(name: impl Into<String>, spec: ToolSpec) -> Self {
        Self {
            name: name.into(),
            spec,
            mutating: false,
            requires_network: false,
            approval_scope: None,
            credential_ref: None,
            cost_estimate_micros: None,
        }
    }

    pub fn with_approval_scope(mut self, scope: impl Into<String>) -> Self {
        self.approval_scope = Some(scope.into());
        self
    }

    pub fn mark_mutating(mut self, scope: impl Into<String>) -> Self {
        self.mutating = true;
        self.approval_scope = Some(scope.into());
        self
    }

    pub fn with_network(mut self, requires_network: bool) -> Self {
        self.requires_network = requires_network;
        self
    }
}

/// Source capability manifest.
#[derive(Clone, Debug)]
pub struct CapabilityManifest {
    pub slug: String,
    pub kind: SourceKind,
    pub trust_tier: TrustTier,
    pub tools: Vec<SourceToolCapability>,
    pub resources: Vec<String>,
    pub prompts: Vec<String>,
    pub filesystem_access: SourceAccess,
    pub network_access: SourceAccess,
    pub credential_access: CredentialAccess,
    pub shared_state: bool,
}

impl CapabilityManifest {
    pub fn new(slug: impl Into<String>, kind: SourceKind, trust_tier: TrustTier) -> Self {
        Self {
            slug: slug.into(),
            kind,
            trust_tier,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
            filesystem_access: SourceAccess::None,
            network_access: SourceAccess::None,
            credential_access: CredentialAccess::none(),
            shared_state: false,
        }
    }

    pub fn with_tool(mut self, tool: SourceToolCapability) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resources.push(resource.into());
        self
    }

    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompts.push(prompt.into());
        self
    }

    pub fn with_filesystem_access(mut self, access: SourceAccess) -> Self {
        self.filesystem_access = access;
        self
    }

    pub fn with_network_access(mut self, access: SourceAccess) -> Self {
        self.network_access = access;
        self
    }

    pub fn with_shared_state(mut self, shared_state: bool) -> Self {
        self.shared_state = shared_state;
        self
    }

    pub fn tool(&self, tool_name: &str) -> Option<&SourceToolCapability> {
        self.tools.iter().find(|tool| tool.name == tool_name)
    }

    pub fn validate(&self) -> Result<(), ManifestValidationError> {
        validate_slug(&self.slug)?;
        for tool in &self.tools {
            validate_tool_name(&tool.name)?;
            if tool.mutating && tool.approval_scope.is_none() {
                return Err(ManifestValidationError::MissingApprovalScope {
                    tool_name: tool.name.clone(),
                });
            }
        }
        Ok(())
    }
}

fn validate_slug(slug: &str) -> Result<(), ManifestValidationError> {
    let mut chars = slug.chars();
    let Some(first) = chars.next() else {
        return Err(ManifestValidationError::InvalidSlug {
            slug: slug.to_string(),
        });
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(ManifestValidationError::InvalidSlug {
            slug: slug.to_string(),
        });
    }
    if chars.any(|ch| !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')) {
        return Err(ManifestValidationError::InvalidSlug {
            slug: slug.to_string(),
        });
    }
    Ok(())
}

fn validate_tool_name(tool_name: &str) -> Result<(), ManifestValidationError> {
    if tool_name.is_empty() || tool_name.contains("__") {
        return Err(ManifestValidationError::InvalidToolName {
            tool_name: tool_name.to_string(),
        });
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestValidationError {
    InvalidSlug { slug: String },
    InvalidToolName { tool_name: String },
    MissingApprovalScope { tool_name: String },
}

impl fmt::Display for ManifestValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlug { slug } => write!(
                f,
                "source manifest slug `{slug}` is invalid; use lowercase letters, digits, or underscores"
            ),
            Self::InvalidToolName { tool_name } => write!(
                f,
                "source manifest tool name `{tool_name}` is invalid; names must be non-empty and may not contain `__`"
            ),
            Self::MissingApprovalScope { tool_name } => write!(
                f,
                "source manifest tool `{tool_name}` mutates state but does not declare an approval scope"
            ),
        }
    }
}

impl std::error::Error for ManifestValidationError {}

#[derive(Debug)]
pub enum SourcePoolError {
    Manifest(ManifestValidationError),
    DuplicateSource {
        slug: String,
    },
    SourceNotFound {
        slug: String,
    },
    ToolNotFound {
        source_slug: String,
        tool_name: String,
    },
    EnablementNotAllowed {
        slug: String,
        trust_tier: TrustTier,
        enablement: SourceEnablement,
    },
    Internal(String),
}

impl fmt::Display for SourcePoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => write!(f, "{error}"),
            Self::DuplicateSource { slug } => write!(f, "source `{slug}` is already registered"),
            Self::SourceNotFound { slug } => write!(f, "source `{slug}` is not registered"),
            Self::ToolNotFound {
                source_slug,
                tool_name,
            } => write!(
                f,
                "source `{source_slug}` does not declare tool `{tool_name}`"
            ),
            Self::EnablementNotAllowed {
                slug,
                trust_tier,
                enablement,
            } => write!(
                f,
                "source `{slug}` with trust tier {trust_tier:?} cannot be enabled through {enablement:?}"
            ),
            Self::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SourcePoolError {}

impl From<ManifestValidationError> for SourcePoolError {
    fn from(value: ManifestValidationError) -> Self {
        Self::Manifest(value)
    }
}

/// Context passed to a source call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCallContext {
    pub session_id: String,
    pub source_slug: String,
    pub tool_name: String,
    pub registered_tool_name: String,
    pub tool_call_id: String,
    pub credential_ref: Option<String>,
    pub state_namespace: String,
}

/// Recorded source-call metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCallRecord {
    pub session_id: String,
    pub source_slug: String,
    pub tool_name: String,
    pub registered_tool_name: String,
    pub tool_call_id: String,
    pub credential_ref: Option<String>,
    pub latency_ms: Option<u128>,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub cost_estimate_micros: Option<u64>,
    pub approval_decision: Option<String>,
    pub state_namespace: String,
    pub success: bool,
}

#[derive(Clone, Default)]
pub struct SourceCallLog {
    records: Arc<Mutex<Vec<SourceCallRecord>>>,
}

impl SourceCallLog {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&self, record: SourceCallRecord) -> Result<(), SourcePoolError> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| SourcePoolError::Internal("source call log lock poisoned".to_string()))?;
        records.push(record);
        Ok(())
    }

    pub fn records(&self) -> Result<Vec<SourceCallRecord>, SourcePoolError> {
        self.records
            .lock()
            .map(|records| records.clone())
            .map_err(|_| SourcePoolError::Internal("source call log lock poisoned".to_string()))
    }
}

#[async_trait]
pub trait Source: Send + Sync {
    fn manifest(&self) -> &CapabilityManifest;

    async fn is_tool_mutating(&self, tool_name: &str, _invocation: &ToolInvocation) -> bool {
        self.manifest()
            .tool(tool_name)
            .map(|tool| tool.mutating)
            .unwrap_or(true)
    }

    async fn requires_network(&self, tool_name: &str, _invocation: &ToolInvocation) -> bool {
        self.manifest()
            .tool(tool_name)
            .map(|tool| tool.requires_network)
            .unwrap_or(true)
    }

    async fn call_tool(
        &self,
        context: SourceCallContext,
        invocation: ToolInvocation,
    ) -> ToolResult<ToolOutput>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceToolNaming {
    Prefixed,
    Legacy,
}

pub fn source_prefixed_tool_name(source_slug: &str, tool_name: &str) -> String {
    format!("{source_slug}__{tool_name}")
}

#[derive(Clone)]
pub struct SourceToolHandler {
    source: Arc<dyn Source>,
    source_slug: String,
    source_tool_name: String,
    registered_tool_name: String,
    session_id: String,
    capability: SourceToolCapability,
    shared_state: bool,
    call_log: SourceCallLog,
}

impl SourceToolHandler {
    pub fn new(
        source: Arc<dyn Source>,
        session_id: impl Into<String>,
        source_tool_name: impl Into<String>,
        registered_tool_name: impl Into<String>,
        call_log: SourceCallLog,
    ) -> Result<Self, SourcePoolError> {
        let source_tool_name = source_tool_name.into();
        let (source_slug, capability, shared_state) = {
            let manifest = source.manifest();
            let capability = manifest.tool(&source_tool_name).cloned().ok_or_else(|| {
                SourcePoolError::ToolNotFound {
                    source_slug: manifest.slug.clone(),
                    tool_name: source_tool_name.clone(),
                }
            })?;
            (manifest.slug.clone(), capability, manifest.shared_state)
        };

        Ok(Self {
            source,
            source_slug,
            source_tool_name,
            registered_tool_name: registered_tool_name.into(),
            session_id: session_id.into(),
            capability,
            shared_state,
            call_log,
        })
    }

    fn state_namespace(&self) -> String {
        if self.shared_state {
            format!("shared:{}", self.source_slug)
        } else {
            format!("session:{}:{}", self.session_id, self.source_slug)
        }
    }

    fn schema_for_registered_name(&self) -> ToolSpec {
        let mut spec = self.capability.spec.clone();
        spec.function.name = self.registered_tool_name.clone();
        spec
    }
}

#[async_trait]
impl ToolHandler for SourceToolHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn is_mutating(&self, invocation: &ToolInvocation) -> bool {
        self.source
            .is_tool_mutating(&self.source_tool_name, invocation)
            .await
    }

    async fn requires_network(&self, invocation: &ToolInvocation) -> bool {
        self.source
            .requires_network(&self.source_tool_name, invocation)
            .await
    }

    async fn handle(&self, invocation: ToolInvocation) -> ToolResult<ToolOutput> {
        let started = Instant::now();
        let mut source_invocation = invocation.clone();
        source_invocation.tool_name = self.source_tool_name.clone();
        let input_bytes = invocation.log_payload().len();
        let context = SourceCallContext {
            session_id: self.session_id.clone(),
            source_slug: self.source_slug.clone(),
            tool_name: self.source_tool_name.clone(),
            registered_tool_name: self.registered_tool_name.clone(),
            tool_call_id: invocation.call_id.clone(),
            credential_ref: self.capability.credential_ref.clone(),
            state_namespace: self.state_namespace(),
        };

        let result = self
            .source
            .call_tool(context.clone(), source_invocation)
            .await;
        let elapsed = started.elapsed().as_millis();
        let output_bytes = result_output_bytes(&result);
        let success = match &result {
            Ok(output) => output.is_success(),
            Err(_) => false,
        };

        let record = SourceCallRecord {
            session_id: context.session_id,
            source_slug: context.source_slug,
            tool_name: context.tool_name,
            registered_tool_name: context.registered_tool_name,
            tool_call_id: context.tool_call_id,
            credential_ref: context.credential_ref,
            latency_ms: Some(elapsed),
            input_bytes,
            output_bytes,
            cost_estimate_micros: self.capability.cost_estimate_micros,
            approval_decision: invocation
                .runtime_context
                .as_ref()
                .and_then(|ctx| ctx.approval.as_ref())
                .map(|approval| format!("{:?}", approval.policy)),
            state_namespace: context.state_namespace,
            success,
        };

        if let Err(error) = self.call_log.record(record)
            && result.is_ok()
        {
            return Err(ToolError::ExecutionFailed(format!(
                "failed to record source tool call: {error}"
            )));
        }

        result
    }

    fn schema(&self) -> ToolSpec {
        self.schema_for_registered_name()
    }
}

fn result_output_bytes(result: &ToolResult<ToolOutput>) -> usize {
    match result {
        Ok(ToolOutput::Function { content, .. }) => content.len(),
        Ok(ToolOutput::Mcp { result }) => result.to_string().len(),
        Err(error) => error.to_string().len(),
    }
}

#[derive(Clone)]
struct SourceRegistration {
    source: Arc<dyn Source>,
    enablement: SourceEnablement,
}

pub type SourceToolHandlers = Vec<(String, Arc<dyn ToolHandler>)>;

#[derive(Clone, Default)]
pub struct SourcePool {
    registrations: Arc<Mutex<HashMap<String, SourceRegistration>>>,
    call_log: SourceCallLog,
}

impl SourcePool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_source(&self, source: Arc<dyn Source>) -> Result<(), SourcePoolError> {
        let enablement = SourceEnablement::default_for_trust_tier(source.manifest().trust_tier);
        self.register_source_with_enablement(source, enablement)
    }

    pub fn register_source_with_enablement(
        &self,
        source: Arc<dyn Source>,
        enablement: SourceEnablement,
    ) -> Result<(), SourcePoolError> {
        let manifest = source.manifest();
        manifest.validate()?;
        validate_enablement(&manifest.slug, manifest.trust_tier, enablement)?;
        let slug = manifest.slug.clone();
        let mut registrations = self
            .registrations
            .lock()
            .map_err(|_| SourcePoolError::Internal("source registry lock poisoned".to_string()))?;
        if registrations.contains_key(&slug) {
            return Err(SourcePoolError::DuplicateSource { slug });
        }
        registrations.insert(slug, SourceRegistration { source, enablement });
        Ok(())
    }

    pub fn enable_source(
        &self,
        slug: &str,
        enablement: SourceEnablement,
    ) -> Result<(), SourcePoolError> {
        let mut registrations = self
            .registrations
            .lock()
            .map_err(|_| SourcePoolError::Internal("source registry lock poisoned".to_string()))?;
        let registration =
            registrations
                .get_mut(slug)
                .ok_or_else(|| SourcePoolError::SourceNotFound {
                    slug: slug.to_string(),
                })?;
        let trust_tier = registration.source.manifest().trust_tier;
        validate_enablement(slug, trust_tier, enablement)?;
        registration.enablement = enablement;
        Ok(())
    }

    pub fn disable_source(&self, slug: &str) -> Result<(), SourcePoolError> {
        self.enable_source(slug, SourceEnablement::Disabled)
    }

    pub fn tool_handlers_for_session(
        &self,
        session_id: impl Into<String>,
        naming: SourceToolNaming,
    ) -> Result<SourceToolHandlers, SourcePoolError> {
        let session_id = session_id.into();
        let registrations = {
            let registrations = self.registrations.lock().map_err(|_| {
                SourcePoolError::Internal("source registry lock poisoned".to_string())
            })?;
            registrations.values().cloned().collect::<Vec<_>>()
        };

        let mut handlers = Vec::new();
        for registration in registrations {
            if !registration.enablement.is_enabled() {
                continue;
            }
            for capability in &registration.source.manifest().tools {
                let registered_name = match naming {
                    SourceToolNaming::Prefixed => source_prefixed_tool_name(
                        &registration.source.manifest().slug,
                        &capability.name,
                    ),
                    SourceToolNaming::Legacy => capability.name.clone(),
                };
                let handler = SourceToolHandler::new(
                    registration.source.clone(),
                    session_id.clone(),
                    capability.name.clone(),
                    registered_name.clone(),
                    self.call_log.clone(),
                )?;
                handlers.push((registered_name, Arc::new(handler) as Arc<dyn ToolHandler>));
            }
        }
        handlers.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(handlers)
    }

    pub fn recorded_calls(&self) -> Result<Vec<SourceCallRecord>, SourcePoolError> {
        self.call_log.records()
    }
}

fn validate_enablement(
    slug: &str,
    trust_tier: TrustTier,
    enablement: SourceEnablement,
) -> Result<(), SourcePoolError> {
    if enablement.is_enabled()
        && trust_tier.requires_explicit_enablement()
        && !enablement.is_explicit()
    {
        return Err(SourcePoolError::EnablementNotAllowed {
            slug: slug.to_string(),
            trust_tier,
            enablement,
        });
    }
    Ok(())
}

impl SourceToolHandler {
    pub fn accepts_payload(payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}
