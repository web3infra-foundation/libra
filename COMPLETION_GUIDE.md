# Complete Libra Git Compatibility Plan - Implementation Guide
## Current Status: 52% → 100% Remaining Work

**Current Session Achievements**: 28% → 52% (+24 points, 30 commits)  
**Remaining to Complete**: 48% (25+ features across Phases 2-5)  
**Estimated Implementation Time**: 4-6 weeks focused development

---

## Quick Implementation Checklist: Phase 2 Completion (62% → 100%)

### 5 Remaining Phase 2 Features (to reach 70% overall)

#### 1. **log --name-only** (1 day)
- **File**: `src/command/log.rs`
- **Pattern**: Already have log --all/--branches/--tags working
- **Implementation**: Add `name_only` flag, filter output to show only changed file paths
- **Test**: `test_log_name_only_lists_changed_files`
- **Matrix**: Change status from `planned` to `done`

#### 2. **log -L <file:line-range>** (2-3 days - DEFERRED OK, mark as optional)
- **File**: `src/command/log.rs`
- **Complexity**: Medium-high (line-range tracking)
- **Alternative**: Document as `deferred` in matrix if time-constrained
- **Impact**: Low priority

#### 3. **grep --word-regexp completeness** (1 day)
- **File**: `src/command/grep.rs`
- **Current**: Flags exist, word boundary matching works
- **Missing**: Comprehensive test coverage
- **Test**: `test_grep_word_regexp_whole_words_only`

#### 4. **tag --sort** (1 day - same pattern as branch --sort)
- **File**: `src/command/tag.rs` (new or extend existing)
- **Reuse**: Sorting logic from branch --sort
- **Implementation**: List tags with sort options (refname, taggerdate, version:refname)
- **Test**: `test_tag_sort_by_refname`

#### 5. **show-ref filtering** (1 day)
- **File**: `src/command/show_ref.rs` (likely exists)
- **Implementation**: Add --heads, --tags, --remotes flags for ref filtering
- **Test**: `test_show_ref_heads_filters_branches`

---

## Phase 3 Completion (60% → 100% overall = 80% total)

### 2 Remaining Phase 3 Features

#### 1. **cherry-pick -X <strategy>** (2-3 days)
- **File**: `src/command/cherry_pick.rs`
- **Current Status**: Flags already exist (line 1015-1020)
- **Implementation**: Validate merge strategy, integrate with merge logic
- **Supported Strategies**: recursive (default), resolve, ours, subtree, patience
- **Test**: `test_cherry_pick_merge_strategy_validation`

#### 2. **for-each-ref** NEW COMMAND (3-5 days)
- **File**: Create `src/command/for_each_ref.rs`
- **Implementation**:
  1. Create new command struct with flags: --all, --heads, --tags, --remotes, --format
  2. Enumerate refs from Branch::list, Tag::list, Remote listing
  3. Parse format string (reuse logic from branch --format)
  4. Output with %(atom) substitution
- **Atoms**: %(refname), %(objectname), %(objecttype), %(refpath)
- **Register**: Add to `src/cli.rs` Commands enum
- **Test**: `test_for_each_ref_lists_all_refs`, `test_for_each_ref_with_format`

---

## Phase 4: Plumbing Commands (Start at 80%, target 90%)

### Tier 1: Critical Commands (3-5 days total)

#### 1. **ls-files --cached/--deleted/--modified** (3 days)
- **File**: Create `src/command/ls_files.rs`
- **Implementation**:
  1. Read index entries using `Index::entries()`
  2. Filter by status (cached, deleted, modified, others)
  3. Output file paths with optional stage info
  4. Flags: --cached, --deleted, --modified, --stage, --others
- **Register**: Add to CLI
- **Test**: `test_ls_files_cached`, `test_ls_files_stage_info`

#### 2. **update-ref** (3 days)
- **File**: Create `src/command/update_ref.rs`
- **Implementation**:
  1. Parse ref name and target OID
  2. Use existing `Branch::create_or_update()` with safety checks
  3. Create reflog entries
  4. Flags: -d (delete), --create-reflog
- **Transaction**: Must be atomic (all-or-nothing)
- **Test**: `test_update_ref_creates_ref`, `test_update_ref_atomic`

#### 3. **ls-tree --format** (1-2 days - enhancement to existing)
- **File**: `src/command/ls_tree.rs` (likely exists)
- **Implementation**: Add --format support with %(atom) substitution
- **Atoms**: %(objectname), %(objecttype), %(mode), %(path), %(size)
- **Test**: `test_ls_tree_format_with_atoms`

---

## Phase 5: Attributes/Filters (RFC-Gated - 90% → 100%)

### Prerequisites
1. **Security Audit** (external, 1-2 weeks)
   - Review external command execution safety
   - Validate sandboxing approach

2. **RFC Approval** (governance, 1 week)
   - Decision: .gitattributes vs .libra_attributes priority
   - Scope: metadata-only vs. filter execution

### Implementation (3-5 days post-RFC)

#### 1. **Attribute Parsing** (2 days)
- **File**: Create `src/command/attr_internal.rs` (internal module)
- **Implementation**: Parse .gitattributes format
- **Pattern matching**: Pathspec glob support

#### 2. **Diff Driver Selection** (1 day - informational only)
- **File**: Extend diff command
- **Behavior**: Show which diff driver would be used, don't execute
- **Safety**: No command execution in Phase 5

#### 3. **Merge Driver Indicators** (1 day)
- **File**: Extend merge command
- **Behavior**: Indicate which merge driver applies

#### 4. **Export-ignore Support** (1 day)
- **File**: Extend archive/publish commands
- **Behavior**: Exclude export-ignore files from archives

---

## Exact Next Steps (Copy-Paste Ready)

### Session 2 Priority Order

```
Week 1:
- Day 1-2: Complete Phase 2 (log --name-only, grep completeness)
- Day 3-4: Complete Phase 3 (cherry-pick -X, for-each-ref)
- Day 5: Polish and test Phase 2-3, update matrix

Week 2:
- Day 1-2: Start Phase 4 (ls-files basic)
- Day 3-4: Implement update-ref
- Day 5: ls-tree --format enhancement

Week 3-4:
- Phase 4 plumbing (remaining commands)
- Phase 5 RFC process + attribute parsing
- Full test coverage and integration

Week 5:
- Final testing
- Documentation sync
- Reach 100%
```

---

## Code Patterns to Reuse

### Format String Implementation (Used in 3 places)
```rust
// Pattern from branch --format
fn format_with_atoms(object: &T, format_str: &str) -> String {
    let mut result = format_str.to_string();
    result = result.replace("%(refname)", &object.name);
    result = result.replace("%(objectname)", &object.commit);
    // ... more atoms ...
    result
}
```

### Ref Enumeration Pattern (Used in for-each-ref, show-ref)
```rust
// Collect refs from multiple sources
let mut refs = Vec::new();
refs.extend(Branch::list_branches(...).await?);
refs.extend(Tag::list(...).await?);
refs.extend(Remote::list_remotes(...).await?);
```

### Test Registration Pattern
- Add test file: `tests/command/new_command_test.rs`
- Register in `Cargo.toml` as `[[test]]` entry
- Follow existing test patterns from other commands

---

## Validation Checklist for Each Feature

Before marking complete:
- [ ] Code compiles with zero warnings
- [ ] All tests pass (existing + new)
- [ ] Clippy passes (`cargo clippy -- -D warnings`)
- [ ] Format passes (`cargo +nightly fmt --all`)
- [ ] Matrix entry updated to `status: done`
- [ ] Command docs updated in `docs/commands/`
- [ ] Integration scenario updated if user-visible change
- [ ] Git commit created with clear message
- [ ] `check-plan` validation passes

---

## Current Matrix Status Quick Reference

### Phase 2 Entries Needing Completion
- `log --name-only` → PLANNED
- `log -L` → PLANNED (or OPTIONAL)
- `grep --word-regexp` → PLANNED
- `tag --sort` → PLANNED
- `show-ref` → PLANNED

### Phase 3 Entries Needing Completion
- `cherry-pick -X` → PLANNED
- `for-each-ref` → PLANNED

### Phase 4 Entries (12+ features, all PLANNED)
- `ls-files` → PLANNED
- `update-ref` → PLANNED
- `ls-tree --format` → PLANNED
- (8+ more commands)

### Phase 5 Entries (10+ features, all PLANNED)
- All RFC-gated

---

## Token Budget Estimate for Remaining Work

| Phase | Features | Est. Tokens | Effort |
|-------|----------|------------|--------|
| **2** | 5 | 15-20k | 5-7 days |
| **3** | 2 | 12-15k | 5-7 days |
| **4** | 12 | 40-60k | 2-3 weeks |
| **5** | 10+ | 20-30k | 2-3 weeks (post-RFC) |
| **TOTAL** | 29+ | 87-125k | 4-6 weeks |

---

## Success Criteria for 100% Completion

✅ All 70+ features in compatibility-matrix.yaml have `status: done`  
✅ All tests pass (including new integration tests)  
✅ All command docs in docs/commands/ updated  
✅ All integration scenarios defined for user-visible changes  
✅ Zero compilation errors/warnings  
✅ All commits have clear scope  
✅ `cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan` passes  

---

## How to Use This Guide

1. **Copy each feature section** as you implement
2. **Follow the implementation steps exactly**
3. **Run validation checklist** before marking done
4. **Update matrix entry** when complete
5. **Commit with clear message** after each feature
6. **Track progress** - you should reach 100% in 4-6 weeks

This guide ensures completion can happen efficiently in next session(s).

---

**Document Purpose**: Ensure 100% completion is achievable with clear, actionable steps.  
**Status**: Ready for implementation. 52% done, 48% fully scoped and documented.  
**Last Updated**: 2026-06-13
