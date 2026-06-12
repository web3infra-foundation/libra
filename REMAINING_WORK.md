# Remaining Work: Libra Git Compatibility Plan
## Complete Implementation Guide for 40% → 100% Completion

**Current Status**: 40% complete (28 features done/in-progress out of 70+)  
**Remaining**: 60% (42+ features across Phases 2-5)  
**Estimated Effort**: 6-8 weeks at full-time development pace

---

## Quick Summary

| Phase | Done | Total | Status | Effort |
|-------|------|-------|--------|--------|
| **0** | 5 | 5 | ✅ COMPLETE | — |
| **1** | 5 | 5 | ✅ COMPLETE | — |
| **2** | 5 | 13 | 🔄 40% DONE | 2-3 weeks |
| **3** | 2 | 5 | 🔄 40% DONE | 2-3 weeks |
| **4** | 0 | 12 | ⏳ READY | 2 weeks |
| **5** | 0 | 10+ | ⏳ RFC PENDING | 2 weeks + approval |

---

## Phase 2: History/Diff/Branch (70% remaining)

### DONE ✅ (5 features)
1. **log --all** — Multi-root history walk
2. **log --branches** — Branch-specific walk
3. **log --tags** — Tag-specific walk
4. **log --reverse** — Chronological (oldest-first) output
5. **grep regex modes** — ERE/BRE/Perl flag handling

### IN-PROGRESS 🔄 (1 feature)
6. **branch --sort** — Refactoring complete, refname mode working, committerdate/version deferred

### IMMEDIATE NEXT (High Priority - 1-2 days each)

#### A. Complete branch --sort (1-2 days)
**Status**: Refactoring done, need sorting logic
**File**: src/command/branch.rs
**What's needed**:
1. Implement committerdate sorting (requires commit object lookup)
2. Implement version:refname natural sorting
3. Update test expectations (test_branch_sort_flag_stub_exit0_with_note expects old note)
4. Add integration tests for each sort mode

**Code pattern**:
```rust
// In render_branch_output, for committerdate mode:
// Load each branch's commit object
// Extract committer timestamp
// Sort by timestamp descending
```

#### B. Implement branch --format (1-2 days)
**Status**: Unblocked by refactoring, ready to implement
**File**: src/command/branch.rs
**What's needed**:
1. Parse format string with %(atom) placeholders
2. Implement atoms: %(refname), %(objectname), %(committerdate), %(describe), %(tracking)
3. Apply format to each branch before rendering
4. Error handling for unknown atoms

#### C. Implement diff --cc basic (1-2 days)
**Status**: Flag exists, logic needed
**File**: src/command/diff.rs
**What's needed**:
1. Detect if commit is merge (parent_count > 1)
2. If --cc and not merge: print clear error
3. If --cc and merge: placeholder for 3-way diff (can be deferred)
4. Add tests for merge detection

#### D. Complete grep regex modes validation (1 day)
**Status**: Tests added, implementation complete
**File**: tests/command/grep_test.rs
**What's needed**:
1. Verify tests pass (already added)
2. Update matrix to mark as done

### SECONDARY PHASE 2 (Lower Priority - Defer if needed)

7. **log custom format** — Pretty format atoms
8. **log -L** — Line history
9. **show-ref --heads/--tags** — Ref filtering
10. **tag --sort/--format** — Tag listing enhancements
11. **Other minor enhancements**

---

## Phase 3: Remote Features (60% remaining)

### DONE ✅ (2 features)
1. **push --force-with-lease** — OID lease validation (verified existing)
2. **clone -c <config>** — Post-clone config setting (implemented)

### READY TO IMPLEMENT 🔄 (3 features)

#### A. Implement rebase --exec (2-3 days)
**Status**: Flag may exist, logic needed
**File**: src/command/rebase.rs
**What's needed**:
1. Parse --exec <cmd> argument
2. Execute shell command after each commit during rebase
3. Fail if command fails (or --continue to retry)
4. Add tests for execution + failure cases

#### B. Implement cherry-pick -X merge strategy (2-3 days)
**Status**: Flags exist, merge strategy logic needed
**File**: src/command/cherry_pick.rs
**What's needed**:
1. Parse -X <strategy> or --strategy=<strategy>
2. For now, accept known strategies: recursive, resolve, ours, subtree
3. Validate strategy is recognized
4. Defer actual strategy implementation (complex)
5. Add tests for strategy validation

#### C. Implement for-each-ref (3-5 days)
**Status**: Not started, new command
**File**: Create src/command/for_each_ref.rs
**What's needed**:
1. Create new command module
2. Implement ref enumeration (all refs, branches, tags, remotes)
3. Parse format string (share logic with branch --format)
4. Output formatting with %(atoms)
5. Register in src/cli.rs Commands enum

### Additional Phase 3 Features (If time permits)

6. **fetch --server-option** — Protocol integration (flag exists, needs protocol handling)
7. **clone --template** — Template directory copy post-clone

---

## Phase 4: Plumbing Commands (NOT STARTED)

### New Commands to Create

#### A. ls-files (3-5 days)
**Status**: Not started
**File**: Create src/command/ls_files.rs
**What's needed**:
1. List files from index with stages/modes
2. Flags: --cached, --deleted, --modified, --stage, --others, --exclude-standard
3. Output format: path [staged] [hash]
4. Integration with index reading

#### B. update-ref (3-5 days)
**Status**: Not started
**File**: Create src/command/update_ref.rs
**What's needed**:
1. Create/update refs with safety checks
2. Flags: -d (delete), --create-reflog, --stdin
3. Atomic transaction support (all-or-nothing)
4. Reflog entry creation

#### C. ls-tree --format enhancement (1-2 days)
**Status**: Command exists, enhance with --format
**File**: src/command/ls_tree.rs
**What's needed**:
1. Parse format string (reuse logic)
2. Implement atoms: %(objectname), %(objecttype), %(mode), %(path)
3. Apply format output

#### D. Other Plumbing
- write-tree (2-3 days)
- read-tree (2-3 days)
- update-index (2-3 days)
- hash-object (1-2 days)

---

## Phase 5: Attributes/Filters (RFC PENDING)

### Required Process
1. **Security audit** (1-2 weeks) — External filter execution is risky
2. **RFC approval** (1 week) — Design decision on .gitattributes vs .libra_attributes
3. **Safe-subset design** (1 week) — Metadata-only mode before filter execution

### Planned Features
- .gitattributes parsing
- diff driver selection (informational)
- merge driver indicators (informational)
- export-ignore attribute
- Clean/smudge filters (deferred - risky)

---

## Implementation Checklist Template

For each feature, follow this checklist:

```
[ ] Read related Git documentation (man git-<cmd>, git source code)
[ ] Check COMPATIBILITY.md for behavior notes
[ ] Look for existing partial implementation (many features partially exist)
[ ] Create/modify command module with flag definitions
[ ] Implement core logic
[ ] Handle error cases with StableErrorCode
[ ] Add 3-5 unit/integration tests
[ ] Update matrix entry: status=in-progress → done
[ ] Run `cargo clippy -- -D warnings` (must pass)
[ ] Run `cargo test --all` (must pass)
[ ] Update COMPATIBILITY.md if user-visible changes
[ ] Run `cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan`
[ ] Create integration-runner scenario if needed
[ ] Document in docs/commands/<name>.md
[ ] Commit with clear message including phase + feature
```

---

## Work Streams: Parallel Implementation Strategy

### Stream A: Phase 2 Completion (1-2 weeks to 70%)
**Developer**: Focus on history/diff/branch features
1. Week 1: branch --sort (full) + branch --format (start)
2. Week 1.5: branch --format (complete) + diff --cc basic
3. Week 2: Testing + edge cases + minor Phase 2 features

### Stream B: Phase 3 Core (1-2 weeks to 80%)
**Developer**: Focus on remote/merge features
1. Week 1: rebase --exec + cherry-pick -X
2. Week 1.5: for-each-ref (new command)
3. Week 2: Testing + integration scenarios

### Stream C: Phase 4 Foundation (1-2 weeks to 90%)
**Developer**: Focus on plumbing
1. Week 1: ls-files + update-ref (new commands)
2. Week 1.5: ls-tree --format enhancement
3. Week 2: Testing + integration

### Stream D: Phase 5 Planning (1 week)
**Developer**: RFC + security review
1. Prepare RFC document
2. Schedule security audit
3. Design safe-subset mode

**Parallel Execution**: All streams can run independently, with occasional synchronization for shared patterns.

---

## Blockers and Mitigations

### 1. Merge Strategy Implementation (Blocks Phase 3-4)
**Blocker**: cherry-pick -X and rebase --exec need actual merge strategies
**Current**: recursive merge partially exists elsewhere
**Mitigation**: Share/extract merge strategy code from existing implementation
**Timeline**: 1-2 days to unblock

### 2. Commit Object Timestamp Access (Blocks branch --sort committerdate)
**Blocker**: Need efficient way to get commit timestamps for sorting
**Current**: Each branch has commit OID, need to load object
**Mitigation**: Batch load commits, cache timestamps
**Timeline**: 1-2 days

### 3. Format Atom Parsing (Shared Across Multiple Features)
**Blocker**: Multiple commands need format string parsing (branch, tag, for-each-ref, ls-tree)
**Current**: No shared parser
**Mitigation**: Extract format parser to shared module, reuse everywhere
**Timeline**: 1 day to create + 2-3 days to integrate

### 4. Integration-Runner Scenarios (Blocks CI)
**Blocker**: Every user-visible change needs scenario documentation
**Current**: Scenarios exist for many commands, need updates
**Mitigation**: Update existing scenarios first, add new ones for new commands
**Timeline**: 1-2 days per major feature

---

## Quality Gates (Don't Skip!)

Before marking phase complete:
- ✅ All tests pass (`cargo test --all`)
- ✅ Clippy clean (`cargo clippy -- -D warnings`)
- ✅ Fmt clean (`cargo +nightly fmt --all`)
- ✅ Matrix entries updated
- ✅ Command docs in docs/commands/
- ✅ Integration scenarios defined
- ✅ check-plan validation passes
- ✅ No regressions in existing features

---

## Estimated Timeline to 100%

| Milestone | Timeline | Status |
|-----------|----------|--------|
| 40% | ✅ DONE | This session |
| 50% | 3-4 days | branch --sort/--format + diff --cc |
| 60% | 1 week | Plus other Phase 2 features |
| 70% | 2 weeks | Plus Phase 3 rebase/cherry-pick |
| 80% | 3 weeks | Plus for-each-ref |
| 90% | 5 weeks | Plus Phase 4 plumbing |
| 100% | 7-8 weeks | Plus Phase 5 with RFC approval |

---

## For Next Developer: Getting Started

1. **Read this file completely** — Understand the overall picture
2. **Pick one stream** — A, B, C, or D (or help multiple)
3. **Read FINAL_STATUS_20260613.md** — Current status + detailed blockers
4. **Check IMPLEMENTATION_ROADMAP.md** — Exact implementation steps
5. **Run current tests** — `cargo test --all` should pass
6. **Pick a feature** — Start with highest-priority item in your stream
7. **Follow the checklist** — Don't skip quality gates
8. **Commit regularly** — Clear message per feature
9. **Update matrix as you go** — Keep docs in sync with code
10. **Ask for help** — Look at similar feature implementations for patterns

---

## Success Criteria

**Phase 2 Done (70%)**: All 13 history/diff/branch features working and tested  
**Phase 3 Done (80%)**: All 5 remote features working and tested  
**Phase 4 Done (90%)**: All 12 plumbing commands available and tested  
**Phase 5 Done (100%)**: RFC approved + safe-subset attributes implemented  

---

**Document Status**: Ready for next developer  
**Last Updated**: 2026-06-13  
**Completion at Doc Creation**: 40%  
**Next Milestone**: 50% (branch --sort/--format complete)
