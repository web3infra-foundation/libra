//! Sub-agent tool-call trace id (CEX-S2-14, Step 2.4).
//!
//! CEX-S2-14 应该完成的功能: "每个 tool call 追加 trace id：`thread_id ->
//! agent_run_id -> tool_call_id -> source_call_id`". This module provides the
//! pure 4-segment trace identifier that threads a tool call (and, when it
//! reaches the Source Pool, its source call) back through the sub-agent run to
//! the originating thread, so per-agent cost / latency attribution and
//! observability can group events by any prefix of the chain.
//!
//! The id is **hierarchical**: the `source_call_id` segment is optional (a tool
//! call that never hits the Source Pool has none), and any prefix is itself a
//! valid grouping key. The canonical string form joins the present segments with
//! `/` so a log line carries the whole lineage at a glance:
//! `thread/<t>/run/<r>/tool/<c>[/source/<s>]`.
//!
//! Pure — construction and rendering only, no I/O.

use uuid::Uuid;

use super::{AgentRunId, SourceCallId, ToolCallId};

/// The hierarchical trace id for one sub-agent tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolCallTraceId {
    /// Parent Layer 1 session thread.
    pub thread_id: Uuid,
    /// The sub-agent run that issued the tool call.
    pub agent_run_id: AgentRunId,
    /// The tool call itself.
    pub tool_call_id: ToolCallId,
    /// The Source Pool call the tool call made, if any. `None` for tool calls
    /// that never reached the Source Pool.
    pub source_call_id: Option<SourceCallId>,
}

impl ToolCallTraceId {
    /// Construct a trace id for a tool call that has **not** (yet) made a Source
    /// Pool call.
    pub fn new(thread_id: Uuid, agent_run_id: AgentRunId, tool_call_id: ToolCallId) -> Self {
        Self {
            thread_id,
            agent_run_id,
            tool_call_id,
            source_call_id: None,
        }
    }

    /// Extend this trace id with the `source_call_id` of a Source Pool call the
    /// tool call made — the trailing segment of the chain. Returns a new id;
    /// the receiver is unchanged.
    pub fn with_source_call(self, source_call_id: SourceCallId) -> Self {
        Self {
            source_call_id: Some(source_call_id),
            ..self
        }
    }

    /// The canonical `/`-joined string form, omitting the trailing
    /// `source/<s>` segment when there is no source call.
    pub fn to_canonical_string(&self) -> String {
        let mut s = format!(
            "thread/{}/run/{}/tool/{}",
            self.thread_id, self.agent_run_id.0, self.tool_call_id.0,
        );
        if let Some(source_call_id) = self.source_call_id {
            s.push_str(&format!("/source/{}", source_call_id.0));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trace_has_no_source_segment() {
        let thread = Uuid::from_u128(1);
        let run = AgentRunId(Uuid::from_u128(2));
        let tool = ToolCallId(Uuid::from_u128(3));
        let trace = ToolCallTraceId::new(thread, run, tool);

        assert_eq!(trace.source_call_id, None);
        let rendered = trace.to_canonical_string();
        assert_eq!(
            rendered,
            format!(
                "thread/{}/run/{}/tool/{}",
                Uuid::from_u128(1),
                Uuid::from_u128(2),
                Uuid::from_u128(3),
            ),
        );
        assert!(
            !rendered.contains("source/"),
            "a tool call with no source call must omit the source segment",
        );
    }

    #[test]
    fn with_source_call_appends_trailing_segment() {
        let trace = ToolCallTraceId::new(
            Uuid::from_u128(1),
            AgentRunId(Uuid::from_u128(2)),
            ToolCallId(Uuid::from_u128(3)),
        )
        .with_source_call(SourceCallId(Uuid::from_u128(4)));

        assert_eq!(trace.source_call_id, Some(SourceCallId(Uuid::from_u128(4))));
        assert_eq!(
            trace.to_canonical_string(),
            format!(
                "thread/{}/run/{}/tool/{}/source/{}",
                Uuid::from_u128(1),
                Uuid::from_u128(2),
                Uuid::from_u128(3),
                Uuid::from_u128(4),
            ),
        );
    }

    #[test]
    fn with_source_call_leaves_other_segments_intact() {
        let base = ToolCallTraceId::new(
            Uuid::from_u128(10),
            AgentRunId(Uuid::from_u128(20)),
            ToolCallId(Uuid::from_u128(30)),
        );
        let extended = base.with_source_call(SourceCallId(Uuid::from_u128(40)));

        assert_eq!(extended.thread_id, base.thread_id);
        assert_eq!(extended.agent_run_id, base.agent_run_id);
        assert_eq!(extended.tool_call_id, base.tool_call_id);
        // The base is unchanged (value semantics).
        assert_eq!(base.source_call_id, None);
    }

    #[test]
    fn canonical_string_is_a_prefix_groupable_chain() {
        // The thread/run prefix of two tool calls in the same run matches, so a
        // log consumer can group by it.
        let thread = Uuid::from_u128(1);
        let run = AgentRunId(Uuid::from_u128(2));
        let a = ToolCallTraceId::new(thread, run, ToolCallId(Uuid::from_u128(3)));
        let b = ToolCallTraceId::new(thread, run, ToolCallId(Uuid::from_u128(9)));

        let prefix = format!("thread/{}/run/{}/tool/", thread, run.0);
        assert!(a.to_canonical_string().starts_with(&prefix));
        assert!(b.to_canonical_string().starts_with(&prefix));
        // Distinct tool segments keep the full ids distinct.
        assert_ne!(a.to_canonical_string(), b.to_canonical_string());
    }
}
