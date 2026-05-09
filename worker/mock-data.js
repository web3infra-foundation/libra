/* global React */
// Mock data — field names match §6 API exactly.
// Keep this file the single source of truth for fixtures.

window.LIBRA_API = (() => {
  // ── Refs (branches + tags, exposed by ref picker) ───────────────────────
  const refs = [
    { name: "main",                 type: "branch", oid: "9a1f3e2c", is_default: true,  protected: true,  ahead: 0,  behind: 0,
      last_commit_at: "2026-05-07T08:55:48Z", last_commit_author: "m.ostrowski", last_commit_summary: "ledger: structured JournalError taxonomy",
      sync_state: "synced",  publish_state: "published", ai_versions_count: 5 },
    { name: "release/2026.05",      type: "branch", oid: "7e22a18b", is_default: false, protected: true,  ahead: 0,  behind: 12,
      last_commit_at: "2026-05-04T14:20:09Z", last_commit_author: "release-bot",   last_commit_summary: "cut release-candidate notes",
      sync_state: "synced",  publish_state: "published", ai_versions_count: 1 },
    { name: "feat/idempotency-key", type: "branch", oid: "b40c91d5", is_default: false, protected: false, ahead: 4,  behind: 2,
      last_commit_at: "2026-05-07T07:18:33Z", last_commit_author: "j.tanaka",      last_commit_summary: "WIP: thread idempotency_key through withRetry",
      sync_state: "syncing", publish_state: "publishing", ai_versions_count: 3 },
    { name: "fix/redaction-policy", type: "branch", oid: "1cf8e602", is_default: false, protected: false, ahead: 1,  behind: 9,
      last_commit_at: "2026-05-05T22:04:11Z", last_commit_author: "m.ostrowski",   last_commit_summary: "tighten dotenv redaction patterns",
      sync_state: "stale",   publish_state: "published", ai_versions_count: 2 },
    { name: "exp/sandboxed-runs",   type: "branch", oid: "55a31fb9", is_default: false, protected: false, ahead: 22, behind: 41,
      last_commit_at: "2026-04-29T11:42:55Z", last_commit_author: "libra-reason-3", last_commit_summary: "spike: in-memory worktrees",
      sync_state: "synced",  publish_state: "private",   ai_versions_count: 7 },
    { name: "chore/upgrade-node20", type: "branch", oid: "3d04a6c1", is_default: false, protected: false, ahead: 0,  behind: 18,
      last_commit_at: "2026-04-22T09:12:01Z", last_commit_author: "ci-bot",         last_commit_summary: "bump node engine to 20.11",
      sync_state: "error",   publish_state: "failed",    ai_versions_count: 0 },
    { name: "v0.4.2",               type: "tag",    oid: "7e22a18b", is_default: false, protected: true,  ahead: null, behind: null,
      last_commit_at: "2026-05-04T14:20:09Z", last_commit_author: "release-bot",    last_commit_summary: "v0.4.2",
      sync_state: "synced",  publish_state: "published", ai_versions_count: 0 },
    { name: "v0.4.1",               type: "tag",    oid: "a91d2244", is_default: false, protected: true,  ahead: null, behind: null,
      last_commit_at: "2026-04-12T18:05:42Z", last_commit_author: "release-bot",    last_commit_summary: "v0.4.1",
      sync_state: "synced",  publish_state: "published", ai_versions_count: 0 },
    { name: "v0.4.0",               type: "tag",    oid: "44b0ee18", is_default: false, protected: true,  ahead: null, behind: null,
      last_commit_at: "2026-03-28T10:11:00Z", last_commit_author: "release-bot",    last_commit_summary: "v0.4.0",
      sync_state: "synced",  publish_state: "published", ai_versions_count: 0 },
    { name: "v0.3.9",               type: "tag",    oid: "0fbb1d77", is_default: false, protected: true,  ahead: null, behind: null,
      last_commit_at: "2026-02-19T16:50:21Z", last_commit_author: "release-bot",    last_commit_summary: "v0.3.9",
      sync_state: "synced",  publish_state: "published", ai_versions_count: 0 },
  ];

  // ── Repository ───────────────────────────────────────────────────────────
  const repository = {
    repo_id: "rp_8f4c1b",
    slug: "kepler-ledger",            // URL-safe; CLI scheme parses host as the slug
    name: "kepler-ledger",
    default_branch: "main",
    head_sha: "9a1f3e2c",
    last_indexed_at: "2026-05-07T09:14:22Z",
    sync_state: "synced",            // synced | syncing | stale | error | paused
    storage_bytes: 184_312_904,
    file_count: 1_247,
    visibility: "private",
    description: "Double-entry ledger service · TypeScript · Node 20",
    refs_summary: { branches: 6, tags: 4, published: 8, failed: 1 },
  };

  // ── Files (tree entries, §6: file_node) ──────────────────────────────────
  // type:  blob | tree
  // status flags surfaced in the UI:
  //   is_binary, is_too_large, is_ignored, has_redactions
  const file_tree = [
    { path: "src",                       type: "tree", file_count: 42 },
    { path: "src/index.ts",              type: "blob", size_bytes: 2_184,  language: "typescript" },
    { path: "src/ledger",                type: "tree", file_count: 18 },
    { path: "src/ledger/journal.ts",     type: "blob", size_bytes: 9_842,  language: "typescript", has_redactions: true },
    { path: "src/ledger/posting.ts",     type: "blob", size_bytes: 4_120,  language: "typescript" },
    { path: "src/ledger/balance.ts",     type: "blob", size_bytes: 7_553,  language: "typescript" },
    { path: "src/ledger/.env",           type: "blob", size_bytes: 412,    language: "dotenv",     has_redactions: true },
    { path: "src/ledger/snapshot.bin",   type: "blob", size_bytes: 4_982_213, is_binary: true },
    { path: "src/migrations",            type: "tree", file_count: 14 },
    { path: "src/migrations/001_init.sql", type: "blob", size_bytes: 1_204, language: "sql" },
    { path: "vendor",                    type: "tree", file_count: 312, is_ignored: true },
    { path: "dist",                      type: "tree", file_count: 87,  is_ignored: true },
    { path: "logs",                      type: "tree", file_count: 9 },
    { path: "logs/2026-04-30.log",       type: "blob", size_bytes: 38_412_660, is_too_large: true, language: "log" },
    { path: "README.md",                 type: "blob", size_bytes: 6_204,  language: "markdown" },
    { path: "package.json",              type: "blob", size_bytes: 1_842,  language: "json" },
    { path: "tsconfig.json",             type: "blob", size_bytes: 612,    language: "json" },
    { path: ".gitignore",                type: "blob", size_bytes: 248,    language: "gitignore" },
  ];

  // ── A specific file (§6: file_object) ────────────────────────────────────
  const file_object__journal = {
    path: "src/ledger/journal.ts",
    blob_sha: "c4d12ea7",
    size_bytes: 9_842,
    language: "typescript",
    is_binary: false,
    is_too_large: false,
    is_ignored: false,
    has_redactions: true,
    redaction_count: 3,
    last_modified_at: "2026-05-06T22:08:11Z",
    last_author: "m.ostrowski",
    line_count: 184,
  };

  // ── AI Versions (§6: ai_version) ─────────────────────────────────────────
  // status: draft | proposed | accepted | rejected | superseded
  // origin: human_prompt | auto_refactor | scheduled_review
  const ai_versions = [
    {
      version_id: "av_01H9K2",
      parent_version_id: null,
      file_path: "src/ledger/journal.ts",
      base_blob_sha: "c4d12ea7",
      created_at: "2026-05-07T08:42:09Z",
      status: "proposed",
      origin: "human_prompt",
      prompt_excerpt: "Tighten error handling in postJournalEntry; surface validation failures with structured codes.",
      diff_stats: { lines_added: 18, lines_removed: 7, hunks: 4 },
      reviewer: null,
      model: "libra-reason-3",
      tokens_in: 4_212, tokens_out: 612,
      confidence: 0.82,
      tests_run: { passed: 41, failed: 0, skipped: 2 },
    },
    {
      version_id: "av_01H9J7",
      parent_version_id: "av_01H9G3",
      file_path: "src/ledger/journal.ts",
      base_blob_sha: "b0a981ff",
      created_at: "2026-05-06T19:11:44Z",
      status: "accepted",
      origin: "auto_refactor",
      prompt_excerpt: "Auto-refactor: extract validation predicates into pure helpers.",
      diff_stats: { lines_added: 34, lines_removed: 22, hunks: 6 },
      reviewer: "m.ostrowski",
      model: "libra-reason-3",
      tokens_in: 3_840, tokens_out: 904,
      confidence: 0.91,
      tests_run: { passed: 43, failed: 0, skipped: 0 },
    },
    {
      version_id: "av_01H9G3",
      parent_version_id: "av_01H9C1",
      file_path: "src/ledger/journal.ts",
      base_blob_sha: "9c14770a",
      created_at: "2026-05-05T14:02:01Z",
      status: "superseded",
      origin: "human_prompt",
      prompt_excerpt: "Add idempotency key support to postJournalEntry.",
      diff_stats: { lines_added: 52, lines_removed: 9, hunks: 7 },
      reviewer: "j.tanaka",
      model: "libra-reason-2",
      tokens_in: 5_104, tokens_out: 1_212,
      confidence: 0.74,
      tests_run: { passed: 39, failed: 2, skipped: 0 },
    },
    {
      version_id: "av_01H9C1",
      parent_version_id: null,
      file_path: "src/ledger/journal.ts",
      base_blob_sha: "771ab003",
      created_at: "2026-05-03T10:55:27Z",
      status: "rejected",
      origin: "scheduled_review",
      prompt_excerpt: "Weekly review: suggest dead-code removals in journal module.",
      diff_stats: { lines_added: 0, lines_removed: 41, hunks: 3 },
      reviewer: "m.ostrowski",
      model: "libra-reason-2",
      tokens_in: 2_140, tokens_out: 188,
      confidence: 0.58,
      tests_run: { passed: 38, failed: 0, skipped: 0 },
    },
    {
      version_id: "av_01H988",
      parent_version_id: null,
      file_path: "src/ledger/journal.ts",
      base_blob_sha: "5b22ca10",
      created_at: "2026-05-01T07:30:14Z",
      status: "draft",
      origin: "human_prompt",
      prompt_excerpt: "Sketch — what would idempotent retries look like?",
      diff_stats: { lines_added: 12, lines_removed: 0, hunks: 2 },
      reviewer: null,
      model: "libra-reason-3",
      tokens_in: 1_840, tokens_out: 320,
      confidence: 0.66,
      tests_run: null,
    },
  ];

  // ── Sync events (§6: sync_event) ─────────────────────────────────────────
  // event_type: index_started | index_completed | webhook_received |
  //             rate_limited | auth_refresh | error | paused
  const sync_events = [
    { event_id: "se_44129", event_type: "index_completed", at: "2026-05-07T09:14:22Z",
      detail: "Indexed 1,247 files (12 changed) in 8.3s", level: "info" },
    { event_id: "se_44128", event_type: "webhook_received", at: "2026-05-07T09:14:11Z",
      detail: "push refs/heads/main · 12 commits · m.ostrowski", level: "info" },
    { event_id: "se_44127", event_type: "auth_refresh",     at: "2026-05-07T06:00:02Z",
      detail: "Installation token rotated (expires in 60m)", level: "info" },
    { event_id: "se_44120", event_type: "rate_limited",     at: "2026-05-06T23:48:00Z",
      detail: "GitHub: 4,872/5,000 used; backed off 90s", level: "warn" },
    { event_id: "se_44115", event_type: "error",            at: "2026-05-06T22:31:09Z",
      detail: "Webhook signature mismatch; dropped 1 event", level: "error" },
    { event_id: "se_44102", event_type: "index_started",    at: "2026-05-06T22:08:11Z",
      detail: "Reindex requested by m.ostrowski", level: "info" },
  ];

  // ── Errors (§6: error_object) ────────────────────────────────────────────
  // Used for the Empty/Error collection.
  const error_examples = [
    { code: "FILE_NOT_FOUND",      http: 404, message: "No file at path src/ledger/old.ts at sha c4d12ea7." },
    { code: "BLOB_TOO_LARGE",      http: 413, message: "Blob is 38.4 MB; viewer limit is 2 MB." },
    { code: "BINARY_NOT_VIEWABLE", http: 415, message: "Binary content cannot be displayed inline." },
    { code: "REPO_NOT_INDEXED",    http: 409, message: "Repository has not completed initial indexing." },
    { code: "RATE_LIMITED",        http: 429, message: "Upstream provider rate limit reached. Try again in 90s." },
    { code: "INTEGRATION_REVOKED", http: 401, message: "GitHub App installation was uninstalled by an admin." },
  ];

  // ── IntentSpec / Agent Object Model ──────────────────────────────────────
  // Mirrors `intent-spec.md` (control plane) + `ai-object-model-reference.md`
  // (snapshot/event/projection split). One thread, focused on the proposed
  // AI version `av_01H9K2` so List + Detail share a coherent narrative.

  // Thread (Libra projection)
  const thread = {
    thread_id: "th_8c12f0",
    title: "Tighten error handling in postJournalEntry",
    owner: { type: "human", id: "m.ostrowski", display_name: "M. Ostrowski" },
    participants: [
      { type: "human", id: "m.ostrowski", role: "owner",     joined_at: "2026-05-07T08:39:11Z" },
      { type: "human", id: "j.tanaka",    role: "reviewer",  joined_at: "2026-05-07T08:41:02Z" },
      { type: "agent", id: "libra-reason-3", role: "agent",  joined_at: "2026-05-07T08:39:14Z" },
    ],
    current_intent_id:  "in_01H9K2_r3",
    latest_intent_id:   "in_01H9K2_r3",
    intents: [
      { intent_id: "in_01H9K2_r1", ordinal: 1, is_head: false, linked_at: "2026-05-07T08:39:18Z", link_reason: "draft"  },
      { intent_id: "in_01H9K2_r2", ordinal: 2, is_head: false, linked_at: "2026-05-07T08:42:01Z", link_reason: "modify" },
      { intent_id: "in_01H9K2_r3", ordinal: 3, is_head: true,  linked_at: "2026-05-07T08:46:33Z", link_reason: "modify" },
    ],
    archived: false,
  };

  // Intent (snapshot) — head revision, with embedded IntentSpec excerpt
  const intent_head = {
    intent_id: "in_01H9K2_r3",
    parents:   ["in_01H9K2_r2"],
    created_at: "2026-05-07T08:46:33Z",
    created_by: { type: "human", id: "m.ostrowski" },
    status: "Active",
    prompt:
      "Tighten error handling in postJournalEntry; surface validation failures with structured codes and " +
      "preserve idempotency on retries.",
    spec: {
      api_version: "intentspec.io/v1alpha1",
      kind: "IntentSpec",
      metadata: {
        id: "is_01H9K2_r3",
        created_at: "2026-05-07T08:46:33Z",
        created_by: { type: "user", id: "m.ostrowski", display_name: "M. Ostrowski" },
        target: {
          repo:    { type: "git", locator: "git@github.com:kepler/ledger.git" },
          base_ref: "9a1f3e2c",
          workspace_id: "kepler-ledger",
          labels: { ticket: "KEP-2841", team: "ledger-core" },
        },
      },
      intent: {
        summary: "Tighten error handling in postJournalEntry",
        change_type: "bugfix",
        objectives: [
          { title: "Return structured error codes for validation failures",     kind: "implementation" },
          { title: "Preserve idempotency_key across retried postings",          kind: "implementation" },
          { title: "Audit current call-sites and document upgrade path",        kind: "analysis" },
        ],
        in_scope:     ["src/ledger/journal.ts", "src/ledger/posting.ts", "tests/ledger/**"],
        out_of_scope: ["src/migrations/**", "vendor/**", "src/ledger/balance.ts"],
        touch_hints: {
          files:   ["src/ledger/journal.ts", "tests/ledger/journal.spec.ts"],
          symbols: ["postJournalEntry", "withRetry", "currentActor"],
          apis:    ["/v2/journal", "/v2/journal/retry"],
        },
      },
      acceptance: {
        success_criteria: [
          "All validation failures return { code, message, retriable } shape.",
          "Same idempotency_key never produces two committed rows.",
          "Existing 41 unit tests continue to pass; ≥3 new tests cover retry path.",
        ],
        verification_plan: {
          fast_checks:        4,
          integration_checks: 2,
          security_checks:    3,
          release_checks:     1,
        },
        quality_gates: {
          require_new_tests_when_bugfix: true,
          max_allowed_regression: "none",
        },
      },
      constraints: {
        security: { network_policy: "deny", dependency_policy: "no-new" },
        privacy:  { data_classes_allowed: ["public", "internal"], redaction_required: true, retention_days: 30 },
        licensing:{ allowed_spdx: ["Apache-2.0", "MIT", "BSD-3-Clause"], forbid_new_licenses: false },
        platform: { language_runtime: "node20", supported_os: ["linux", "darwin"] },
        resources:{ max_wall_clock_seconds: 14_400, max_cost_units: 25_000 },
      },
      risk: {
        level: "medium",
        rationale:
          "Touches the write-path of the ledger but is bounded by an idempotency_key. " +
          "No schema or migration changes; existing test surface is dense.",
        factors: ["write-path", "retries", "ledger-core"],
        human_in_loop: { required: true, min_approvers: 1 },
      },
      evidence: {
        strategy: "repo-first",
        trust_tiers: ["repo", "standards", "vendor-doc"],
        domain_allowlist_mode: "allowlist-only",
        allowed_domains: ["docs.libra.dev", "node.js.org", "*.github.io"],
        blocked_domains: ["*"],
        min_citations_per_decision: 1,
      },
      security: {
        tool_acl: {
          allow: [
            { tool: "workspace.fs",      actions: ["read", "write"], constraints: { write_roots: ["src/ledger/", "tests/ledger/"] } },
            { tool: "workspace.command", actions: ["execute"],       constraints: { allow_commands: ["pnpm test", "pnpm lint"] } },
            { tool: "workspace.lsp",     actions: ["hover", "goto-definition", "find-references"] },
            { tool: "workspace.search",  actions: ["read"] },
          ],
          deny: [
            { tool: "workspace.command", actions: ["execute"], constraints: { deny_substrings: ["curl ", "wget ", "rm -rf"] } },
          ],
        },
        secrets:           { policy: "deny-all", allowed_scopes: [] },
        prompt_injection:  { treat_retrieved_content_as_untrusted: true, enforce_output_schema: true, disallow_instruction_from_evidence: true },
        output_handling:   { encoding_policy: "contextual-escape", no_direct_eval: true },
      },
      execution: {
        retry:       { max_retries: 3, backoff_seconds: 10 },
        replan:      { triggers: ["repeated-test-fail", "security-gate-fail", "evidence-conflict", "scope-creep"] },
        concurrency: { max_parallel_tasks: 2 },
      },
      artifacts: {
        required: [
          { name: "patchset",                stage: "per-task",    required: true, format: "git-diff" },
          { name: "test-log",                stage: "per-task",    required: true, format: "junit+xml" },
          { name: "sast-report",             stage: "security",    required: true, format: "sarif" },
          { name: "sca-report",              stage: "security",    required: true, format: "cyclonedx-json" },
          { name: "sbom",                    stage: "security",    required: true, format: "spdx-json" },
          { name: "provenance-attestation",  stage: "release",     required: true, format: "in-toto+json" },
          { name: "transparency-proof",      stage: "release",     required: true, format: "rekor-inclusion-proof" },
        ],
        retention: { days: 90 },
      },
      provenance: {
        require_slsa_provenance: true,
        require_sbom: true,
        transparency_log: { mode: "rekor" },
        bindings: { embed_intentspec_digest: true, embed_evidence_digests: true },
      },
      lifecycle: {
        schema_version: "1.0.0",
        status: "active",
        change_log: [
          { at: "2026-05-07T08:39:18Z", by: "m.ostrowski",  reason: "draft",                      diff_summary: "Initial intent draft." },
          { at: "2026-05-07T08:42:01Z", by: "libra-reason-3", reason: "modify · readonly analysis", diff_summary: "Added idempotency objective; tightened in_scope." },
          { at: "2026-05-07T08:46:33Z", by: "m.ostrowski",  reason: "modify · review",            diff_summary: "Required structured error codes; reviewer set." },
        ],
      },
    },
  };

  // Plan pair (snapshot) — exactly one execution + one test plan
  const plans = [
    {
      plan_id: "pl_K2_exec_v2",
      kind: "execution",
      intent_id: "in_01H9K2_r3",
      parents: ["pl_K2_exec_v1"],
      created_at: "2026-05-07T08:51:07Z",
      steps: [
        { step_id: "ps_e1", title: "Introduce JournalError taxonomy",            depends_on: [],         status: "completed" },
        { step_id: "ps_e2", title: "Refactor postJournalEntry to emit codes",    depends_on: ["ps_e1"],  status: "completed" },
        { step_id: "ps_e3", title: "Thread idempotency_key through withRetry",   depends_on: ["ps_e2"],  status: "running"   },
        { step_id: "ps_e4", title: "Update call-sites in posting.ts",            depends_on: ["ps_e2"],  status: "pending"   },
      ],
    },
    {
      plan_id: "pl_K2_test_v1",
      kind: "test",
      intent_id: "in_01H9K2_r3",
      parents: [],
      created_at: "2026-05-07T08:51:07Z",
      steps: [
        { step_id: "ps_t1", title: "Unit: validation → structured codes",        depends_on: [],         status: "pending" },
        { step_id: "ps_t2", title: "Unit: idempotency_key dedup",                depends_on: [],         status: "pending" },
        { step_id: "ps_t3", title: "Integration: retry under flaky DB",          depends_on: ["ps_t2"],  status: "pending" },
        { step_id: "ps_t4", title: "Security: SAST + SCA gates",                 depends_on: [],         status: "pending" },
      ],
    },
  ];

  // Tasks (snapshot) — work units with provenance to plan steps
  const tasks = [
    { task_id: "tk_e1", origin_step_id: "ps_e1", parent_task_id: null,    intent_id: "in_01H9K2_r3", goal: "bugfix",
      title: "Introduce JournalError taxonomy", dependencies: [], constraints: ["in-scope: src/ledger/", "out-of-scope: vendor/**"],
      status: "completed", run_count: 1 },
    { task_id: "tk_e2", origin_step_id: "ps_e2", parent_task_id: null,    intent_id: "in_01H9K2_r3", goal: "bugfix",
      title: "Refactor postJournalEntry to emit codes", dependencies: ["tk_e1"], constraints: ["in-scope: src/ledger/journal.ts"],
      status: "completed", run_count: 2 },
    { task_id: "tk_e3", origin_step_id: "ps_e3", parent_task_id: null,    intent_id: "in_01H9K2_r3", goal: "bugfix",
      title: "Thread idempotency_key through withRetry", dependencies: ["tk_e2"], constraints: ["in-scope: src/ledger/journal.ts"],
      status: "running", run_count: 1 },
    { task_id: "tk_e4", origin_step_id: "ps_e4", parent_task_id: null,    intent_id: "in_01H9K2_r3", goal: "bugfix",
      title: "Update call-sites in posting.ts", dependencies: ["tk_e2"], constraints: ["in-scope: src/ledger/posting.ts"],
      status: "pending", run_count: 0 },
  ];

  // Runs (snapshot) — execution attempts. The latest run produced av_01H9K2.
  const runs = [
    { run_id: "rn_e2_a",   task_id: "tk_e2", plan_id: "pl_K2_exec_v2", commit: "9a1f3e2c", started_at: "2026-05-07T08:53:11Z", finished_at: "2026-05-07T08:55:48Z", duration_seconds: 157, status: "succeeded", patchset_count: 2, retry_index: 0 },
    { run_id: "rn_e2_b",   task_id: "tk_e2", plan_id: "pl_K2_exec_v2", commit: "9a1f3e2c", started_at: "2026-05-07T08:57:02Z", finished_at: "2026-05-07T08:58:30Z", duration_seconds:  88, status: "succeeded", patchset_count: 1, retry_index: 1 },
    { run_id: "rn_e3_a",   task_id: "tk_e3", plan_id: "pl_K2_exec_v2", commit: "9a1f3e2c", started_at: "2026-05-07T09:01:14Z", finished_at: null,                     duration_seconds: 622, status: "running",   patchset_count: 1, retry_index: 0 },
    { run_id: "rn_e1_a",   task_id: "tk_e1", plan_id: "pl_K2_exec_v2", commit: "9a1f3e2c", started_at: "2026-05-07T08:51:30Z", finished_at: "2026-05-07T08:52:48Z", duration_seconds:  78, status: "succeeded", patchset_count: 1, retry_index: 0 },
  ];

  // PatchSet (snapshot)
  const patchsets = [
    { patchset_id: "ps_av_01H9K2", run_id: "rn_e3_a", sequence: 1, format: "git-diff",
      commit: "9a1f3e2c", touched: ["src/ledger/journal.ts"], lines_added: 18, lines_removed: 7, hunks: 4,
      rationale: "Wrap insert in withRetry; thread idempotency_key; replace literal actor with currentActor()." },
    { patchset_id: "ps_av_e2b",   run_id: "rn_e2_b", sequence: 1, format: "git-diff",
      commit: "9a1f3e2c", touched: ["src/ledger/journal.ts"], lines_added: 22, lines_removed: 11, hunks: 3,
      rationale: "Emit JournalError(code, message, retriable); collapse two return paths." },
  ];

  // Provenance (snapshot) — model + provider parameters
  const provenance_head = {
    provenance_id: "pv_K2_e3a",
    run_id: "rn_e3_a",
    provider: "libra-cloud",
    model:    "libra-reason-3",
    parameters: {
      temperature: 0.2,
      top_p: 0.9,
      max_output_tokens: 2048,
      seed: 17,
      external_parameters: { intentspec_digest: "sha256:f1c2…9b3a" },
    },
    builder_id: "libra-scheduler@2026.05.06",
    intentspec_digest: "sha256:f1c2…9b3a",
    slsa_level: "L3",
  };

  // RunUsage (event)
  const run_usage = [
    { usage_id: "ru_e1_a", run_id: "rn_e1_a", tokens_in:   980, tokens_out:  220, cost_usd: 0.0042, wall_clock_seconds:  78 },
    { usage_id: "ru_e2_a", run_id: "rn_e2_a", tokens_in: 1_840, tokens_out:  410, cost_usd: 0.0091, wall_clock_seconds: 157 },
    { usage_id: "ru_e2_b", run_id: "rn_e2_b", tokens_in:   612, tokens_out:  140, cost_usd: 0.0029, wall_clock_seconds:  88 },
    { usage_id: "ru_e3_a", run_id: "rn_e3_a", tokens_in:   780, tokens_out: 0,    cost_usd: 0.0033, wall_clock_seconds: 622 },
  ];

  // Events (append-only)
  const intent_events = [
    { event_id: "ie_01", at: "2026-05-07T08:39:18Z", intent_id: "in_01H9K2_r1", actor: "m.ostrowski",  kind: "drafted" },
    { event_id: "ie_02", at: "2026-05-07T08:42:01Z", intent_id: "in_01H9K2_r1", actor: "libra-reason-3", kind: "modified", next_intent_id: "in_01H9K2_r2" },
    { event_id: "ie_03", at: "2026-05-07T08:46:33Z", intent_id: "in_01H9K2_r2", actor: "m.ostrowski",  kind: "modified", next_intent_id: "in_01H9K2_r3" },
    { event_id: "ie_04", at: "2026-05-07T08:46:34Z", intent_id: "in_01H9K2_r3", actor: "m.ostrowski",  kind: "confirmed" },
  ];

  const tool_invocations = [
    { invocation_id: "ti_201", run_id: "rn_e2_a", at: "2026-05-07T08:53:14Z", tool: "workspace.search", action: "read",
      args_summary: 'symbol: "postJournalEntry"',                exit_code: 0,  io_footprint: { paths_read: ["src/ledger/journal.ts"] } },
    { invocation_id: "ti_202", run_id: "rn_e2_a", at: "2026-05-07T08:53:42Z", tool: "workspace.fs",     action: "read",
      args_summary: '"src/ledger/journal.ts"',                   exit_code: 0,  io_footprint: { paths_read: ["src/ledger/journal.ts"] } },
    { invocation_id: "ti_203", run_id: "rn_e2_a", at: "2026-05-07T08:54:11Z", tool: "workspace.fs",     action: "write",
      args_summary: '"src/ledger/journal.ts" · 1 hunk',          exit_code: 0,  io_footprint: { paths_written: ["src/ledger/journal.ts"] } },
    { invocation_id: "ti_204", run_id: "rn_e2_a", at: "2026-05-07T08:54:48Z", tool: "workspace.command", action: "execute",
      args_summary: "pnpm test ledger/journal",                  exit_code: 0,  io_footprint: { paths_read: ["tests/ledger/"] } },
    { invocation_id: "ti_205", run_id: "rn_e3_a", at: "2026-05-07T09:01:31Z", tool: "workspace.lsp",    action: "find-references",
      args_summary: 'symbol: "withRetry"',                       exit_code: 0,  io_footprint: { paths_read: ["src/ledger/"] } },
    { invocation_id: "ti_206", run_id: "rn_e3_a", at: "2026-05-07T09:02:10Z", tool: "workspace.fs",     action: "write",
      args_summary: '"src/ledger/journal.ts" · 4 hunks',         exit_code: 0,  io_footprint: { paths_written: ["src/ledger/journal.ts"] } },
  ];

  const evidence = [
    { evidence_id: "ev_e1_lint",  run_id: "rn_e1_a", at: "2026-05-07T08:52:30Z", kind: "lint",     status: "passed",
      summary: "eslint: 0 errors, 0 warnings", artifact: { name: "lint-log",  format: "text" } },
    { evidence_id: "ev_e2_unit",  run_id: "rn_e2_a", at: "2026-05-07T08:55:40Z", kind: "tests",    status: "passed",
      summary: "41/43 passed · 2 skipped",     artifact: { name: "test-log",  format: "junit+xml" } },
    { evidence_id: "ev_e2_sast",  run_id: "rn_e2_b", at: "2026-05-07T08:58:25Z", kind: "sast",     status: "passed",
      summary: "0 high · 1 info (allowed)",    artifact: { name: "sast-report", format: "sarif" } },
    { evidence_id: "ev_e3_unit",  run_id: "rn_e3_a", at: "2026-05-07T09:11:12Z", kind: "tests",    status: "running",
      summary: "running · 12/43 so far",       artifact: null },
  ];

  const decisions = [
    { decision_id: "dc_e2",  run_id: "rn_e2_b", at: "2026-05-07T08:58:42Z", kind: "patchset_selected",
      chosen_patchset_id: "ps_av_e2b", actor: "libra-scheduler",
      rationale: "Lowest diff against base; passes all per-task gates." },
    { decision_id: "dc_int", run_id: "rn_e2_b", at: "2026-05-07T08:58:55Z", kind: "intent_advanced",
      actor: "libra-scheduler",
      rationale: "Stage barrier: execution_dag step 2 complete; advance to step 3." },
  ];

  const context_frames = [
    { frame_id: "cf_001", at: "2026-05-07T08:39:21Z", kind: "intent_analysis", protected: true,
      intent_id: "in_01H9K2_r1", trust: "repo",
      summary: "Located postJournalEntry; identified 4 call-sites; no migrations involved." },
    { frame_id: "cf_002", at: "2026-05-07T08:42:08Z", kind: "evidence",        protected: false,
      intent_id: "in_01H9K2_r2", trust: "repo",
      summary: "Existing tests cover happy-path only; idempotency tests missing." },
    { frame_id: "cf_003", at: "2026-05-07T08:51:09Z", kind: "plan_context",    protected: false,
      plan_id: "pl_K2_exec_v2", trust: "repo",
      summary: "Decomposed by objective; 4 execution steps + 4 test steps; no cross-plan edges." },
    { frame_id: "cf_004", at: "2026-05-07T08:53:18Z", kind: "tool_result",     protected: false,
      run_id: "rn_e2_a", step_id: "ps_e2", trust: "repo",
      summary: "Located JournalEntry shape and DB driver insert signature." },
    { frame_id: "cf_005", at: "2026-05-07T09:01:24Z", kind: "checkpoint",      protected: true,
      run_id: "rn_e3_a", trust: "repo",
      summary: "Pre-replan checkpoint before threading idempotency_key." },
  ];

  // Scheduler projection (Libra)
  const scheduler = {
    selected_plan_ids:    ["pl_K2_exec_v2", "pl_K2_test_v1"],
    current_plan_heads:   ["pl_K2_exec_v2", "pl_K2_test_v1"],
    active_task_id:       "tk_e3",
    active_run_id:        "rn_e3_a",
    active_dag_stage:     "execution",
    ready_queue:          ["tk_e4"],
    parallel_groups:      [["tk_e3", "tk_e4"]],
    live_context_window:  ["cf_001", "cf_003", "cf_004", "cf_005"],
  };

  return {
    refs,
    repository,
    file_tree,
    file_object__journal,
    ai_versions,
    sync_events,
    error_examples,
    // Agent object model
    thread,
    intent_head,
    plans,
    tasks,
    runs,
    patchsets,
    provenance_head,
    run_usage,
    intent_events,
    tool_invocations,
    evidence,
    decisions,
    context_frames,
    scheduler,
  };
})();
