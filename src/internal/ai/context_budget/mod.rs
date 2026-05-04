//! Provider-aware context budget planning and allocation.
//!
//! CEX-13a defined the segment budget contract and deterministic allocation
//! behavior. CEX-13b layers append-only context frames, attachment references,
//! and compaction replay records on top of that core. CEX-13c adds reviewed
//! memory anchors for cross-turn semantic constraints.

pub mod allocator;
pub mod budget;
pub mod compaction;
pub mod frame;
pub mod memory_anchor;

pub use allocator::{
    AllocationOmissionReason, ContextAllocation, ContextAllocationOmission, ContextBudgetAllocator,
    ContextBudgetCandidate,
};
pub use budget::{
    ContextBudget, ContextBudgetError, ContextPriority, ContextSegmentBudget, ContextSegmentKind,
    ProviderContextCapability, TruncationPolicy,
};
pub use compaction::{CompactionEvent, CompactionReason};
pub use frame::{
    ContextAttachmentRef, ContextAttachmentStore, ContextFrameBuilder, ContextFrameCandidate,
    ContextFrameEvent, ContextFrameKind, ContextFrameOmission, ContextFrameSegment,
    ContextFrameSource, ContextFrameSourceKind, ContextTrustLevel,
};
pub use memory_anchor::{
    MemoryAnchor, MemoryAnchorAction, MemoryAnchorConfidence, MemoryAnchorDraft, MemoryAnchorEvent,
    MemoryAnchorKind, MemoryAnchorLookupError, MemoryAnchorReplay, MemoryAnchorReviewState,
    MemoryAnchorScope, build_memory_anchor_prompt_section,
};
