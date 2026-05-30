//! Guard `docs/improvement/agent.md` against stale implementation claims.
//!
//! The Agent plan is an active backlog document. Several sections are historical
//! by design, but the current-risk table must not claim removed provider
//! surfaces still exist after the source and CLI migration tests have closed
//! them.

const AGENT_DOC: &str = include_str!("../../docs/improvement/agent.md");
const CODE_COMMAND: &str = include_str!("../../src/command/code.rs");

#[test]
fn agent_doc_keeps_claudecode_marked_removed_not_active() {
    assert!(
        !CODE_COMMAND.contains("CodeProvider::Claudecode"),
        "src/command/code.rs must not reintroduce the removed claudecode provider variant",
    );

    for forbidden in [
        "`claudecode` provider 仍存在",
        "code.rs` 仍有 Claudecode provider",
    ] {
        assert!(
            !AGENT_DOC.contains(forbidden),
            "docs/improvement/agent.md must not keep stale claudecode-active claim: {forbidden}",
        );
    }

    assert!(
        AGENT_DOC.contains("claudecode 硬删除"),
        "agent.md should continue to describe claudecode as a completed hard-delete wave",
    );
    assert!(
        AGENT_DOC.contains("`src/internal/ai/claudecode/` 不存在"),
        "agent.md should keep the source-grounded removal evidence",
    );
}
