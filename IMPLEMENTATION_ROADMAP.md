# Libra Git Compatibility - Complete Implementation Roadmap

**Current Status**: 25% complete (Phase 0-1 done, Phase 2-5 partially started)
**Goal**: 100% completion of all content in docs/development/compatibility.md
**Estimated Remaining**: 4-7 weeks of systematic implementation

---

## Phase 2: History/Diff/Branch Queries (~40 flags, 20% done)

### COMPLETED ✅
- `log --all` - Multi-root history walk (fully implemented)
- `log --branches` - Branch-only walk (fully implemented)
- `log --tags` - Tag-only walk (fully implemented)

### IMMEDIATE NEXT (Priority: High)

#### 1. branch --sort Implementation (1-2 days)
**File**: `src/command/branch.rs:1602`
**Current**: Prints "not implemented" note, ignores sort flag
**TODO**:
1. Parse sort key in `render_branch_output()` (line 1618)
2. Implement sorting logic:
   - `refname` - alphabetical by branch name
   - `committerdate` - by last commit date (descending default)
   - `version:refname` - natural sort order
3. Apply sort to branches vec before rendering
4. Remove "not implemented" note (line 1603-1605)
5. Add test case: `branch_list_sort_by_refname`, `branch_list_sort_by_date`
6. Update matrix: `branch --sort` status=done

**Test vectors**:
- `libra branch --sort=refname` - alphabetical output
- `libra branch --sort=committerdate` - newest first
- Default (no sort) - current behavior unchanged

#### 2. branch --format Implementation (1-2 days)
**File**: `src/command/branch.rs:1602`
**Current**: Prints "not implemented" note, ignores format flag
**TODO**:
1. Define format atoms (git format template):
   - `%(refname)` → branch name
   - `%(objectname)` → commit hash (short)
   - `%(committerdate)` → commit date
2. Parse format string in `render_branch_output()`
3. For each branch, substitute atoms in template
4. Print formatted output
5. Remove "not implemented" note
6. Add test: `branch_format_with_atoms`
7. Update matrix: status=done

**Test vectors**:
- `libra branch --format="%(refname) %(objectname)"` - name + hash
- Edge case: unknown atom → error
- Edge case: empty format string → error

#### 3. log --reverse Implementation (2-3 days)
**File**: `src/command/log.rs`
**TODO**:
1. Add flag to LogArgs: `pub reverse: bool`
2. In `execute_safe()`, after collecting commits, reverse the vector if flag set
3. Note: Already sorting by timestamp desc, just need reverse option
4. Add test: `log_reverse_shows_oldest_first`
5. Update matrix: status=done

#### 4. diff --cc Implementation (3-5 days - MEDIUM RISK)
**File**: `src/command/diff.rs`
**Current**: Flag exists (line 170: `pub combined: bool`)
**TODO**:
1. Check if commit is merge (parent_commit_ids.len() > 1)
2. If --cc and not merge: print "not a merge commit" error
3. If --cc and merge:
   - Load all parent trees
   - Perform 3-way+ diff logic
   - Output combined diff format (CC prefix in hunks)
4. Add test: `diff_cc_on_merge_commit`, `diff_cc_on_non_merge_fails`
5. Add git test reference (t4202 combined diff)
6. Update matrix: status=done

**Risk**: Combined diff algorithm is complex; consider simplified version first

#### 5. grep --extended-regexp / --basic-regexp (2-3 days)
**File**: `src/command/grep.rs`
**TODO**:
1. Add regex-mode flags to GrepArgs
2. Pass to Regex::new() with appropriate RegexBuilder options
3. Add test for POSIX extended vs basic regex
4. Update matrix: status=done

### DEFERRED TO LATER PHASE 2 WORK
- `log -L` (line tracing - complex, high risk)
- `log` pretty format atoms (many atoms, low priority)
- Grep advanced regex dialects
- Show-ref / for-each-ref format atoms

---

## Phase 3: Remote Client Interop (~20 flags, 0% done)

### IMMEDIATE (First 3 items enable most critical remote features)

#### 1. push --force-with-lease Implementation (2-3 days)
**File**: `src/command/push.rs:127`
**Current**: Flag defined, marked "lease check uses remote-tracking ref OID only"
**TODO**:
1. Parse force-with-lease value: bare, `=<refname>`, `=<refname>:<expect-oid>`
2. In push execution (line `execute_safe`), before force-push:
   - Get remote tracking ref OID (from FETCH_HEAD or remote)
   - If --force-with-lease: check remote OID matches expected
   - If mismatch: reject ("remote has changed, use --force to override")
3. Add test: `push_force_with_lease_rejects_changed_remote`
4. Update matrix: status=done

**Test vectors**:
- `libra push --force-with-lease` - check remote unchanged
- `libra push --force-with-lease=refs/heads/main` - specify ref
- `libra push --force-with-lease=main:abc123` - specify expected OID
- Remote changed case: error with hint

#### 2. push --signed Implementation (2-3 days)
**File**: `src/command/push.rs:150`
**Current**: SignedPushValue enum defined
**TODO**:
1. When --signed is set:
   - If value is "true": sign push certificate with GPG/vault
   - If value is "false": skip signing
   - If value is "if-asked": sign only if server supports
2. Integrate with existing `vault_sign_commit()` pattern
3. Add certificate to push negotiation
4. Add test: `push_signed_includes_gpg_signature`
5. Update matrix: status=done

#### 3. fetch --server-option Implementation (1-2 days)
**File**: `src/command/fetch.rs:597` (just added)
**TODO**:
1. Pass server_option vec to protocol layer (git-protocol.rs)
2. In protocol negotiation, advertise server options
3. Collect acknowledgments in response
4. Add test: `fetch_server_option_passed_to_remote`
5. Update matrix: status=done

### SECONDARY (Enable full remote compatibility)

#### 4. clone -c <key>=<value> Implementation (1-2 days)
**File**: `src/command/clone.rs`
**TODO**:
1. Add `-c` flag to CloneArgs
2. After clone completes, apply each config value via ConfigKv
3. Add test: `clone_c_applies_config_after_clone`

#### 5. clone --template <path> Implementation (2-3 days)
**File**: `src/command/clone.rs`
**TODO**:
1. Copy template directory contents post-clone
2. Handle Git template scripts
3. Add test: `clone_template_copies_template_files`

---

## Phase 4: Plumbing Commands (~12 new commands, 0% done)

### TIER 1 - CRITICAL (Enable automation)

#### 1. ls-files Implementation (3-5 days)
**File**: Create `src/command/ls_files.rs`
**Scope**: List tracked files with modes
**Flags**:
- `--cached` - show index contents
- `--deleted` - show deleted tracked files
- `--modified` - show modified tracked files
- `--stage` - show index stage info
**TODO**:
1. Create command module with LsFilesArgs struct
2. Implement file listing from index (Index::entries())
3. Filter by flags (cached, deleted, modified)
4. Format output per mode
5. Register in src/cli.rs Commands enum
6. Add test: `ls_files_lists_tracked_files`, `ls_files_stage_shows_mode`
7. Update matrix: status=done

#### 2. update-ref Implementation (3-5 days)
**File**: Create `src/command/update_ref.rs`
**Scope**: Update/create refs with safety checks
**Flags**:
- `-d` / `--delete` - delete ref
- `--create-reflog` - create reflog entry
- `--stdin` - read ops from stdin
**TODO**:
1. Parse ref name and target OID
2. Use Branch::create_or_update() with checks
3. Handle atomic transactions (all-or-nothing)
4. Add test: `update_ref_creates_ref`, `update_ref_fails_if_locked`
5. Register in CLI
6. Update matrix: status=done

**Risk**: Transaction semantics - must prove ACID

#### 3. ls-tree Implementation (Enhancement, 1-2 days)
**File**: `src/command/ls_tree.rs` (already exists)
**TODO**:
1. Add `--format` support (already defined flag)
2. Parse format atoms: `%(objectname)`, `%(mode)`, `%(path)`
3. Update matrix: status=done

---

## Phase 5: Attributes/Filters (~10 items, 0% done, HIGH RISK)

### DEFERRED - Requires RFC approval (docs/improvement/attributes-rfc.md)

**Entry Requirements**:
- Security audit for external command execution
- Design doc on .libra_attributes vs .gitattributes priority
- Safe-subset implementation (metadata only, no execution)

**Planned Scope**:
1. Attribute parsing from .gitattributes
2. Diff driver selection (informational only, no execution in Phase 5)
3. Merge driver indicators (no execution)
4. Export-ignore attribute for archives

---

## Implementation Checklist Template

For each feature, follow this checklist:

```
[ ] Flag/struct definition
[ ] Core logic implementation
[ ] Test cases (golden path + edge cases)
[ ] Git test source reference (e.g., t4202)
[ ] Command documentation update (docs/commands/<cmd>.md)
[ ] COMPATIBILITY.md update (add decline ref if rejecting)
[ ] Matrix update (status=in-progress → done)
[ ] Integration scenario (if user-visible behavior change)
[ ] Check-plan validation passes
[ ] Clippy/fmt clean
```

---

## Parallel Work Streams

### Stream 1: Phase 2 Completion (2-3 weeks)
1. branch --sort + --format (4-5 days)
2. log --reverse (2-3 days)
3. diff --cc (3-5 days)
4. grep regex modes (2-3 days)
5. Testing & integration (3-4 days)

### Stream 2: Phase 3 Foundation (1-2 weeks)
1. push --force-with-lease (2-3 days)
2. push --signed (2-3 days)
3. fetch --server-option (1-2 days)
4. clone -c (1-2 days)
5. Integration testing (2-3 days)

### Stream 3: Phase 4 Start (1-2 weeks)
1. ls-files (3-5 days)
2. update-ref (3-5 days)
3. ls-tree --format (1-2 days)
4. Testing & registration (2-3 days)

### Stream 4: Phase 5 Planning (1 week)
- RFC review process
- Security audit
- Safe-subset design

---

## Quality Gates Per Phase

Before marking phase as "done":
- ✅ All matrix entries status=done OR blocked-with-reason
- ✅ All new StableErrorCode in docs/error-codes.md
- ✅ All command docs in docs/commands/
- ✅ All integration scenarios in integration-scenarios.yaml
- ✅ `cargo clippy -- -D warnings` passes
- ✅ `cargo +nightly fmt --all` passes
- ✅ All tests pass (including compat guards)
- ✅ `cargo run --manifest-path tools/integration-runner/Cargo.toml -- check-plan` passes
- ✅ `cargo run -- check-plan` shows no unclassified entries

---

## Timeline to 100% Completion

| Phase | Effort | Timeline | Status |
|-------|--------|----------|--------|
| **0** | Done | ✅ | 100% complete |
| **1** | Done | ✅ | 100% complete |
| **2** | 2-3w | 🔄 In progress (20% done) |
| **3** | 1-2w | ⏳ Ready to start |
| **4** | 1-2w | ⏳ Ready to start |
| **5** | 1-2w | ⏳ RFC pending |
| **TOTAL** | 4-7w | ~75% remaining |

---

## Critical Success Factors

1. **Flag definitions often exist** - Search for `pub <name>: <type>` in Args structs before adding new flags
2. **Test patterns proven** - Replicate existing test structure (golden path + edge cases + git ref)
3. **Matrix tracks everything** - Update matrix status after each implementation
4. **Integration scenarios mandatory** - Every user-visible change must map to integration-runner scenario
5. **Quality gates non-negotiable** - Don't skip clippy/fmt/tests; they catch real bugs

---

## Next Immediate Actions (Next 4-8 hours)

1. Implement branch --sort (parsing + sorting logic)
2. Implement branch --format (atom substitution)
3. Add tests for both
4. Update matrix entries to status=done
5. Commit with clear message tracking progress

Then:
6. Implement log --reverse (simpler, quick win)
7. Implement diff --cc basic (merge detection + error handling for non-merges)
8. Move to Phase 3 (push --force-with-lease most critical)

---

## Success Metrics

**Phase 2 Completion**: All 40+ flags working (log/diff/grep/branch/tag enhancements)
**Phase 3 Completion**: All 20+ remote client features working
**Phase 4 Completion**: All 12 plumbing commands available and tested
**Phase 5 Completion**: Attributes/filters RFC approved and safe-subset implemented

**100% Completion**: All 70+ parameters implemented, tested, documented, and integrated with zero regressions

---

Generated: 2026-06-13
Next review: After Phase 2 completion
