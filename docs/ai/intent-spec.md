# IntentSpec 设计

> 本地实现说明（2026-03-13）
>
> Libra 目前在 `intent.objectives[]` 中接受结构化目标，
> 其中每个 objective 为 `{ title, kind }`，`kind` 取
> `implementation` 或 `analysis` 之一。

**Version:** 1.0.0  
**Spec foundations:** JSON Schema Draft 2020-12 · NIST SSDF · SLSA v1.0 · OWASP LLM Top 10 (2025)  
**Execution layer:** Libra AI Object Model (`git-internal`)

---

## 目录

1. [设计哲学与架构分层](#1-设计哲学与架构分层)
2. [完整 JSON Schema（含 Libra 扩展）](#2-完整-json-schema含-libra-扩展)
3. [字段参考](#3-字段参考)
4. [字段速查表](#4-字段速查表)
5. [示例 1 —— 最小化（低风险 bugfix）](#5-示例-1--最小化低风险-bugfix)
6. [示例 2 —— 典型（中风险新功能）](#6-示例-2--典型中风险新功能)
7. [示例 3 —— 高保障（高风险安全修复）](#7-示例-3--高保障高风险安全修复)
8. [示例参数对比](#8-示例参数对比)

---

## 1. 设计哲学与架构分层

IntentSpec 是一份**机器可读的意图契约（intent contract）**。它把一段自然语言请求转化为结构化、可验证的输入，供调度器（Scheduler）进行调度、门禁（gate）与审计。它不是一个 prompt——而是一份契约，承载着：

- **Intent** —— 要做什么、不要做什么，以及可接受的结果应该是什么样子
- **Constraints** —— 围绕安全、隐私、许可与资源的硬性边界
- **Gates** —— 流水线每个阶段推进前必须通过的检查
- **Evidence policy** —— 从何处获取信息，以及对每个来源的可信度（trust）有多高
- **Provenance bindings** —— 如何以密码学方式将意图、执行与最终产物链接起来

在 Libra 系统内部，IntentSpec 位于**控制平面（control plane）**；Libra AI Object Model 位于**执行平面（execution plane）**：

```
IntentSpec  (control plane)
     │  drives
     ▼
Libra: Intent → Plan → Task DAG → Run → PatchSet → Evidence → Decision
     │  produces
     ▼
git commit + SBOM + attestation + Rekor proof
```

在本文档中，运行时首选术语为**调度器（Scheduler）**。现有的 schema 与兼容性字段（如 `orchestratorActorId`）作为遗留名称保留原位，直至引入明确的迁移方案为止。

### 标准对齐

| Standard | IntentSpec 如何使用它 |
|---|---|
| **NIST SSDF** | `artifacts.required` 与 `provenance.*` 实现了 PS.3.2（收集/维护/共享溯源（provenance）数据，例如 SBOM） |
| **SLSA v1.0** | `provenance.bindings.embedIntentSpecDigest` 将 IntentSpec 置为 `externalParameter`；`transparencyLog.mode=rekor` 满足透明日志（transparency log）的要求 |
| **OWASP LLM Top 10 (2025)** | `security.toolAcl` → Excessive Agency；`evidence.domainAllowlistMode` → Prompt Injection；`security.outputHandling` → Improper Output Handling；`constraints.resources` → Unbounded Consumption |

---

## 2. 完整 JSON Schema（含 Libra 扩展）

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "urn:libra:intentspec:v1",
  "title": "IntentSpec (Libra Edition)",
  "description": "Machine-readable AI code-change intent contract. Drives the Scheduler to produce a Task DAG, execute verification gates, and bind provenance artifacts.",
  "type": "object",
  "required": [
    "apiVersion", "kind", "metadata", "intent", "acceptance",
    "constraints", "risk", "evidence", "security", "execution",
    "artifacts", "provenance", "lifecycle"
  ],
  "unevaluatedProperties": false,
  "properties": {
    "apiVersion": {
      "type": "string",
      "description": "IntentSpec API version, controls Scheduler compatibility routing. Bump the major version on breaking changes (v1→v2). Decoupled from lifecycle.schemaVersion: apiVersion routes, schemaVersion governs field evolution.",
      "pattern": "^intentspec\\.io/v[0-9]+(alpha[0-9]+|beta[0-9]+)?$",
      "default": "intentspec.io/v1alpha1"
    },
    "kind": {
      "type": "string",
      "const": "IntentSpec",
      "description": "Resource type discriminator, fixed to IntentSpec."
    },
    "metadata": { "$ref": "#/$defs/Metadata" },
    "intent":   { "$ref": "#/$defs/Intent" },
    "acceptance": { "$ref": "#/$defs/Acceptance" },
    "constraints": { "$ref": "#/$defs/Constraints" },
    "risk":     { "$ref": "#/$defs/Risk" },
    "evidence": { "$ref": "#/$defs/EvidencePolicy" },
    "security": { "$ref": "#/$defs/SecurityPolicy" },
    "execution": { "$ref": "#/$defs/ExecutionPolicy" },
    "artifacts": { "$ref": "#/$defs/Artifacts" },
    "provenance": { "$ref": "#/$defs/ProvenancePolicy" },
    "lifecycle": { "$ref": "#/$defs/Lifecycle" },
    "libra": {
      "$ref": "#/$defs/LibraBinding",
      "description": "Libra AI Object Model binding config. Controls AI object storage, ContextPipeline frame management, Plan generation strategy, Run execution detail, and Decision policy. All sub-fields default when omitted."
    },
    "extensions": {
      "type": "object",
      "description": "Controlled extension fields for custom metadata that does not affect core gate decisions. Keys should use a 'vendor.io/key' prefix to avoid conflicts.",
      "additionalProperties": true,
      "default": {}
    }
  },

  "$defs": {
    "Metadata": {
      "type": "object",
      "description": "Audit anchor and provenance identity. All fields are immutable after creation.",
      "required": ["id", "createdAt", "createdBy", "target"],
      "additionalProperties": false,
      "properties": {
        "id": {
          "type": "string",
          "description": "Globally unique identifier. Prefer UUIDv4 (random) or ULID (time-ordered). Written to Libra Intent.external_ids[\"intentspec_id\"] and embedded in Provenance as an SLSA externalParameter.",
          "minLength": 8,
          "maxLength": 128
        },
        "createdAt": {
          "type": "string",
          "format": "date-time",
          "description": "Creation timestamp (RFC 3339, timezone required). Maps to Libra Intent.header.created_at."
        },
        "createdBy": {
          "type": "object",
          "description": "Creator identity, maps to a Libra ActorRef.",
          "required": ["type", "id"],
          "additionalProperties": false,
          "properties": {
            "type": {
              "type": "string",
              "enum": ["user", "agent", "system"],
              "description": "Creator type. user → ActorRef::human(); agent → ActorRef::agent(); system → ActorRef::system()."
            },
            "id": { "type": "string", "minLength": 1, "maxLength": 128,
                    "description": "Creator identifier (username, email, agent name, or service ID)." },
            "displayName": { "type": "string", "maxLength": 256,
                             "description": "Optional display name. No effect on business logic." }
          }
        },
        "target": {
          "type": "object",
          "description": "Target repository and baseline. The Scheduler clones via repo.locator and resolves baseRef to a commit SHA written to Libra Run.commit.",
          "required": ["repo", "baseRef"],
          "additionalProperties": false,
          "properties": {
            "repo": {
              "type": "object",
              "required": ["type", "locator"],
              "additionalProperties": false,
              "properties": {
                "type": { "type": "string", "enum": ["git", "monorepo", "local"],
                          "description": "Repository type. git = standard Git remote; monorepo = Piper/Bazel-style large monorepo (Mega); local = local path (dev/test)." },
                "locator": { "type": "string", "minLength": 1, "maxLength": 512,
                             "description": "Repository address: SSH/HTTPS URL for git; //path/to/pkg for monorepo; absolute path for local." }
              }
            },
            "baseRef": {
              "type": "string", "minLength": 1, "maxLength": 128,
              "description": "Change baseline: branch name, tag, or full commit SHA. A full SHA is recommended for high-fidelity provenance."
            },
            "workspaceId": { "type": "string", "maxLength": 128,
                             "description": "Optional workspace identifier for multi-project isolation." },
            "labels": {
              "type": "object",
              "additionalProperties": { "type": "string", "maxLength": 128 },
              "default": {},
              "description": "Free-form key-value labels, written to Libra Intent.header.tags. Common uses: ticket IDs, change numbers, team identifiers."
            }
          }
        }
      }
    },

    "Intent": {
      "type": "object",
      "description": "Structured expression of user intent. The Scheduler derives the Task DAG from this field and enforces scope-creep detection throughout execution. Maps to Libra Intent.prompt/content.",
      "required": ["summary", "problemStatement", "changeType", "objectives", "inScope", "outOfScope"],
      "additionalProperties": false,
      "properties": {
        "summary": {
          "type": "string", "minLength": 5, "maxLength": 256,
          "description": "One-sentence goal, used as Libra Task.title and Intent.content prefix. Written to PR/MR titles and audit logs."
        },
        "problemStatement": {
          "type": "string", "minLength": 10, "maxLength": 8000,
          "description": "Background and problem description. Written verbatim to Libra Intent.prompt (immutable). Include: current symptoms, impact, trigger conditions, and ticket references."
        },
        "changeType": {
          "type": "string",
          "enum": ["bugfix", "feature", "refactor", "performance", "security", "docs", "chore", "unknown"],
          "default": "unknown",
          "description": "Change classification, maps to Libra Task.goal (GoalType). Influences qualityGates defaults and commit message prefix generation."
        },
        "objectives": {
          "type": "array", "minItems": 1,
          "description": "Concrete goals — each maps to one PlanStep and child Task. Each objective should be independently verifiable: state an observable success condition.",
          "items": { "type": "string", "minLength": 3, "maxLength": 2000 }
        },
        "inScope": {
          "type": "array", "minItems": 1,
          "description": "Scope of allowed changes. Written to Libra Task.constraints (prefix 'in-scope:'). The Scheduler checks ToolInvocation io_footprint.paths_written against this list; violations trigger scope-creep replan or rejection.",
          "items": { "type": "string", "minLength": 1, "maxLength": 2000 }
        },
        "outOfScope": {
          "type": "array", "default": [],
          "description": "Explicitly disallowed areas. Written to Task.constraints (prefix 'out-of-scope:'). Prevents the agent from opportunistically modifying adjacent code.",
          "items": { "type": "string", "minLength": 1, "maxLength": 2000 }
        },
        "touchHints": {
          "type": "object",
          "description": "Hints for touch-point localisation. files/symbols/apis are used by the Scheduler to perform static repository searches (ripgrep/ctags/LSP) and generate Libra ContextSnapshot.items[].",
          "additionalProperties": false, "default": {},
          "properties": {
            "files":   { "type": "array", "default": [],
                         "description": "File glob patterns (e.g. 'src/auth/**'). Matched files become ContextItem(File) entries; their blob hashes are used for SLSA resolvedDependencies.",
                         "items": { "type": "string", "minLength": 1, "maxLength": 512 } },
            "symbols": { "type": "array", "default": [],
                         "description": "Code symbol names (functions, classes, methods). The Scheduler locates definitions/references via ctags/LSP and creates ContextItem(Snippet) frames.",
                         "items": { "type": "string", "minLength": 1, "maxLength": 256 } },
            "apis":    { "type": "array", "default": [],
                         "description": "API endpoint paths (e.g. '/v2/report'). The Scheduler looks up OpenAPI spec files and creates ContextItem(Url) frames, domain-checking against allowedDomains.",
                         "items": { "type": "string", "minLength": 1, "maxLength": 512 } }
          }
        }
      }
    },

    "Acceptance": {
      "type": "object",
      "description": "Definition of Done (DoD). Translates 'success' into executable verification steps so gate decisions have objective criteria rather than relying on agent discretion. Maps to Libra PlanStep.checks and expected Evidence states.",
      "required": ["successCriteria", "verificationPlan"],
      "additionalProperties": false,
      "properties": {
        "successCriteria": {
          "type": "array", "minItems": 1,
          "description": "Human-readable acceptance criteria. Written to Libra Task.acceptance_criteria[]. Each item should be observable and verifiable.",
          "items": { "type": "string", "minLength": 5, "maxLength": 4000 }
        },
        "verificationPlan": {
          "type": "object",
          "description": "Four-stage verification plan. Stages execute serially: fastChecks → integrationChecks → securityChecks → releaseChecks. Failure in an earlier stage prevents entry to subsequent stages.",
          "required": ["fastChecks", "integrationChecks", "securityChecks", "releaseChecks"],
          "additionalProperties": false,
          "properties": {
            "fastChecks":        { "$ref": "#/$defs/CheckList",
                                   "description": "Fast checks (<10 min): unit tests, type checks, linting. Run after each PatchSet. Corresponds to Libra per-task Evidence." },
            "integrationChecks": { "$ref": "#/$defs/CheckList",
                                   "description": "Integration checks (10–60 min): contract tests, E2E tests, performance regression. Run after all implementation Tasks complete." },
            "securityChecks":    { "$ref": "#/$defs/CheckList",
          "description": "Security checks: SAST, SCA, secrets scanning. Must produce sast-report, sca-report, sbom artifacts." },
            "releaseChecks":     { "$ref": "#/$defs/CheckList",
                                   "description": "Release checks: human approval (require-approvers), provenance verification. All must pass before Decision.Commit." }
          }
        },
        "qualityGates": {
          "type": "object", "additionalProperties": false, "default": {},
          "description": "Meta-policy constraints that complement explicit commands in verificationPlan.",
          "properties": {
            "requireNewTestsWhenBugfix": {
              "type": "boolean", "default": true,
              "description": "When changeType=bugfix, fastChecks must include at least one new test case; absence is treated as gate fail."
            },
            "maxAllowedRegression": {
              "type": "string", "enum": ["none", "low", "medium"], "default": "none",
              "description": "Maximum allowed performance/coverage regression in integrationChecks. none=zero tolerance; low=within 5%; medium=within 20%."
            }
          }
        }
      }
    },

    "CheckList": {
      "type": "array", "minItems": 0, "items": { "$ref": "#/$defs/Check" }, "default": []
    },

    "Check": {
      "type": "object",
      "description": "A single verification step. Maps to a Libra PlanStep (before execution) and an Evidence record (after execution).",
      "required": ["id", "kind", "required"],
      "additionalProperties": false,
      "properties": {
        "id":      { "type": "string", "minLength": 1, "maxLength": 128,
                     "description": "Unique identifier within this IntentSpec. Used in Evidence.kind labels and gate-decision reports." },
        "kind":    { "type": "string", "enum": ["command", "testSuite", "policy"], "default": "command",
                     "description": "Check type. command=run shell command and check exit code; testSuite=run test framework and parse results; policy=OPA/Rego rule or human-approval gate." },
        "command": { "type": "string", "minLength": 1, "maxLength": 2000,
                     "description": "Command string. The Scheduler validates against security.toolAcl.allow before creating a ToolInvocation. $ENV_VAR references are resolved by the Scheduler, not read from the IntentSpec." },
        "timeoutSeconds": { "type": "integer", "minimum": 1, "maximum": 86400, "default": 900,
                            "description": "Timeout in seconds. Exceeded → gate fail. Recommended: fastChecks<600, integrationChecks<3600, securityChecks<7200." },
        "expectedExitCode": { "type": "integer", "minimum": 0, "maximum": 255, "default": 0,
                              "description": "Expected process exit code. The Scheduler compares Evidence.exit_code to this value." },
        "required": { "type": "boolean", "default": true,
                      "description": "true=failure fails the entire gate stage; false=failure is reported but does not block." },
        "artifactsProduced": {
          "type": "array", "default": [],
          "description": "Artifact names produced by this check. Must match entries in artifacts.required[].name. Missing artifacts fail the gate.",
          "items": { "type": "string", "minLength": 1, "maxLength": 128 }
        }
      }
    },

    "Constraints": {
      "type": "object",
      "description": "Hard constraints: security, privacy, licensing, platform, resources. These are enforced boundaries, not advisory settings. Any violation is a gate fail.",
      "required": ["security", "privacy", "licensing", "platform", "resources"],
      "additionalProperties": false,
      "properties": {
        "security": {
          "type": "object",
          "required": ["networkPolicy", "dependencyPolicy"],
          "additionalProperties": false,
          "properties": {
            "networkPolicy": {
              "type": "string", "enum": ["deny", "allow"], "default": "deny",
              "description": "Network access policy. deny (default) = the Scheduler rejects any ToolInvocation involving external network access (curl, wget, direct npm install, etc.) unless explicitly white-listed in toolAcl with a stated reason."
            },
            "dependencyPolicy": {
              "type": "string", "enum": ["no-new", "allow-with-review", "allow"], "default": "allow-with-review",
              "description": "Dependency introduction policy. no-new=any new dependency detected by SCA is a gate fail; allow-with-review=permitted but requires sca-report for human review; allow=unrestricted (prototype use only)."
            },
            "cryptoPolicy": {
              "type": "string", "maxLength": 2000, "default": "",
              "description": "Free-text cryptographic algorithm constraints (e.g. 'Prohibit custom crypto; use audited library interfaces only'). Written to Task.constraints[] and may be referenced by SAST rule descriptions."
            }
          }
        },
        "privacy": {
          "type": "object",
          "required": ["dataClassesAllowed", "redactionRequired"],
          "additionalProperties": false,
          "properties": {
            "dataClassesAllowed": {
              "type": "array",
              "items": { "type": "string", "enum": ["public", "internal", "confidential", "pii", "phi", "secrets"] },
              "default": ["public"],
              "description": "Allowed data classification levels during code generation, evidence collection, and logging. The Scheduler filters ContextSnapshot items and redacts any content exceeding these classes."
            },
            "redactionRequired": {
              "type": "boolean", "default": true,
              "description": "true = the Scheduler applies a redaction pipeline before writing any ArtifactRef content."
            },
            "retentionDays": {
              "type": "integer", "minimum": 0, "maximum": 3650, "default": 30,
              "description": "Artifact retention period (days) from Decision.Commit. 0=delete immediately. The lower of this value and artifacts.retention.days is used."
            }
          }
        },
        "licensing": {
          "type": "object",
          "required": ["allowedSpdx", "forbidNewLicenses"],
          "additionalProperties": false,
          "properties": {
            "allowedSpdx": {
              "type": "array", "minItems": 0,
              "items": { "type": "string", "minLength": 1, "maxLength": 128 },
              "default": [],
              "description": "Allowed SPDX licence identifiers (e.g. [\"Apache-2.0\", \"MIT\"]). Empty = no restriction. New dependencies must carry a licence in this list; violation is a SCA gate fail."
            },
            "forbidNewLicenses": {
              "type": "boolean", "default": false,
              "description": "true = even SPDX-allowed licences that are not already present in the codebase require a human licence-review step in releaseChecks."
            }
          }
        },
        "platform": {
          "type": "object", "additionalProperties": false, "default": {},
          "properties": {
            "languageRuntime": { "type": "string", "maxLength": 128, "default": "",
                                 "description": "Runtime identifier (e.g. 'node20', 'python3.11', 'rust-1.75'). Used to select the test container image and validate Run.environment." },
            "supportedOS": { "type": "array", "items": { "type": "string", "maxLength": 64 }, "default": [],
                             "description": "Supported OS list (e.g. [\"linux\", \"darwin\"]). Empty = no restriction. The agent should avoid OS-specific syscalls not in this list." }
          }
        },
        "resources": {
          "type": "object", "additionalProperties": false, "default": {},
      "description": "Resource budget — both a cost control and a security control against Unbounded Consumption. The Scheduler must actively enforce these fields.",
          "properties": {
            "maxWallClockSeconds": {
              "type": "integer", "minimum": 1, "maximum": 604800, "default": 14400,
              "description": "Maximum wall-clock time (seconds) for the entire IntentSpec execution. Default 4 hours. Exceeded → Decision.Abandon. Also drives ContextPipeline.max_frames calculation."
            },
            "maxCostUnits": {
              "type": "integer", "minimum": 0, "default": 0,
              "description": "Maximum cost budget (maps to Provenance.token_usage.cost_usd accumulation). 0 = unlimited (trusted internal systems only). Exceeded → reduce parallelism and checkpoint before deciding whether to continue."
            }
          }
        }
      }
    },

    "Risk": {
      "type": "object",
      "description": "Risk classification and human-approval policy. risk.level is not just a label — it drives Scheduler behaviour. The Scheduler validates the consistency of level with humanInLoop in the semantic-validation step.",
      "required": ["level", "rationale", "humanInLoop"],
      "additionalProperties": false,
      "properties": {
        "level": {
          "type": "string", "enum": ["low", "medium", "high"], "default": "medium",
          "description": "Risk level. low=input validation, docs, side-effect-free chores; medium=new features, refactors, data-path changes; high=security fixes, auth/authz changes, crypto, release-blocking defects. Scheduler rule: high requires humanInLoop.required=true and minApprovers>=2."
        },
        "rationale": {
          "type": "string", "minLength": 5, "maxLength": 4000,
          "description": "Justification for the chosen level. Should cover: impact analysis, potential failure modes, and reasoning for not choosing a higher or lower level. Written to Task.constraints[] as audit context."
        },
        "factors": {
          "type": "array", "default": [],
          "description": "Specific risk factor tags (e.g. [\"authz\", \"cve\", \"release-blocking\"]). Schedulers may use these to auto-configure additional checks.",
          "items": { "type": "string", "maxLength": 256 }
        },
        "humanInLoop": {
          "type": "object",
          "required": ["required", "minApprovers"],
          "additionalProperties": false,
          "properties": {
            "required": {
              "type": "boolean", "default": false,
              "description": "true = the Scheduler must await a human-approval signal (PR approval, change-order approval, etc.) before Decision.Commit."
            },
            "minApprovers": {
              "type": "integer", "minimum": 0, "maximum": 10, "default": 0,
              "description": "Minimum number of approvers. 0=none; 1=at least one; 2=four-eyes principle. High-risk changes should set this to ≥2."
            }
          }
        }
      }
    },

    "EvidencePolicy": {
      "type": "object",
      "description": "Evidence sourcing and trust policy. Controls where the Scheduler fetches information (repository, official docs, internet) and how much each source is trusted. The first line of defence against prompt injection: restricts external content to a controlled boundary.",
      "required": ["strategy", "trustTiers", "domainAllowlistMode"],
      "additionalProperties": false,
      "properties": {
        "strategy": {
          "type": "string", "enum": ["repo-first", "pinned-official", "open-web"], "default": "repo-first",
          "description": "Information retrieval priority. repo-first=prefer code/comments/docs within the target repository (safest, minimal external network); pinned-official=allow white-listed official documentation domains; open-web=allow any URL in allowedDomains (highest flexibility, highest risk). Maps to Libra ContextSnapshot.selection_strategy."
        },
        "trustTiers": {
          "type": "array", "minItems": 1,
          "items": { "type": "string", "enum": ["repo", "vendor-doc", "standards", "web", "user-provided"] },
          "default": ["repo", "standards", "vendor-doc"],
          "description": "Allowed evidence trust tiers (descending priority). The Scheduler checks source trust tier when enqueuing ContextItems and tags each with tags[\"trust_tier\"]."
        },
        "domainAllowlistMode": {
          "type": "string", "enum": ["disabled", "allowlist-only"], "default": "allowlist-only",
          "description": "Domain allowlist enforcement. allowlist-only=only domains in allowedDomains may be accessed (recommended); disabled=no domain restriction."
        },
        "allowedDomains": {
          "type": "array",
          "items": { "type": "string", "minLength": 1, "maxLength": 256 },
          "default": [],
          "description": "Permitted domains for Url-type ContextItems. Checked before enqueuing (exact match or *.domain wildcard). List only genuinely required official documentation domains."
        },
        "blockedDomains": {
          "type": "array",
          "items": { "type": "string", "minLength": 1, "maxLength": 256 },
          "default": [],
          "description": "Explicitly blocked domains used to further restrict allowedDomains. When domainAllowlistMode=allowlist-only and blockedDomains=[\"*\"], only domains explicitly listed in allowedDomains are reachable; all other domains are blocked."
        },
        "minCitationsPerDecision": {
          "type": "integer", "minimum": 0, "maximum": 20, "default": 1,
          "description": "Minimum evidence citations required per key technical decision (algorithm choice, dependency selection, interface design). 0=no requirement; 3=recommended for high-risk changes."
        }
      }
    },

    "SecurityPolicy": {
      "type": "object",
      "description": "Tool ACL, data handling, secret access, and output safety policy. The core 'runtime security' section of IntentSpec — addresses OWASP Excessive Agency, Sensitive Information Disclosure, and Improper Output Handling.",
      "required": ["toolAcl", "secrets", "promptInjection", "outputHandling"],
      "additionalProperties": false,
      "properties": {
        "toolAcl": {
          "type": "object",
          "required": ["allow"],
          "additionalProperties": false,
          "description": "Tool Access Control List. The Scheduler checks ACL before creating each ToolInvocation: deny rules first (deny takes priority), then allow rules. Any tool call not in the allow list is rejected.",
          "properties": {
            "allow": {
              "type": "array", "minItems": 1,
              "items": { "$ref": "#/$defs/ToolRule" },
              "description": "Permitted tool rules. Minimum-closure principle: only authorise tools and actions genuinely required for this change."
            },
            "deny": {
              "type": "array",
              "items": { "$ref": "#/$defs/ToolRule" },
              "default": [],
              "description": "Explicitly denied tool rules (priority over allow). Typically used for fine-grained denial (e.g. allow workspace.command but deny commands containing 'curl')."
            }
          }
        },
        "secrets": {
          "type": "object",
          "required": ["policy"],
          "additionalProperties": false,
          "description": "Secret/credential access policy. Consistent with SLSA requirements: signing secret material must only be visible to the build service account, not to agent-executed Run environments.",
          "properties": {
            "policy": {
              "type": "string", "enum": ["deny-all", "read-only-scoped", "allow-scoped"], "default": "deny-all",
              "description": "Secret access policy. deny-all (default)=no secrets injected into Run environment; read-only-scoped=read-only access to named scopes; allow-scoped=read-write to named scopes (requires explicit justification and elevated approval)."
            },
            "allowedScopes": {
              "type": "array",
              "items": { "type": "string", "maxLength": 128 },
              "default": [],
              "description": "Allowed secret scope identifiers (names or prefixes in the platform secret manager). Only effective when policy != deny-all."
            }
          }
        },
        "promptInjection": {
          "type": "object",
          "required": ["treatRetrievedContentAsUntrusted", "enforceOutputSchema"],
          "additionalProperties": false,
          "description": "Prompt injection defence. Isolates external retrieval content from the instruction channel.",
          "properties": {
            "treatRetrievedContentAsUntrusted": {
              "type": "boolean", "default": true,
              "description": "true = all externally retrieved content (URLs, search results, external filesystem files) is tagged tags[\"trust\"]=\"untrusted\" in ContextFrames, and the LLM system prompt explicitly notes the content is untrusted and must not be treated as instructions."
            },
            "enforceOutputSchema": {
              "type": "boolean", "default": true,
              "description": "true = the Scheduler structurally validates every LLM response (PatchSet format, Evidence format, etc.). Invalid structure is treated as failure and triggers Retry rather than being used as-is."
            },
            "disallowInstructionFromEvidence": {
              "type": "boolean", "default": true,
              "description": "true = Evidence summaries are filtered to remove known injection patterns ('Ignore previous instructions', '[SYSTEM]', etc.) before being injected into the next LLM prompt."
            }
          }
        },
        "outputHandling": {
          "type": "object",
          "required": ["encodingPolicy", "noDirectEval"],
          "additionalProperties": false,
          "description": "LLM output handling policy. Prevents generated code containing direct execution patterns (eval/exec/system) and prevents output injection into web/template contexts.",
          "properties": {
            "encodingPolicy": {
              "type": "string", "enum": ["contextual-escape", "strict-json", "none"], "default": "contextual-escape",
              "description": "Output encoding strategy. contextual-escape=select appropriate escaping based on output context (HTML/SQL/Shell); strict-json=all output must be valid JSON; none=no additional encoding."
            },
            "noDirectEval": {
              "type": "boolean", "default": true,
              "description": "true = the Scheduler performs AST-level scanning of all PatchSets to detect and reject eval(), exec(), subprocess(shell=True), os.system(), and equivalent patterns. Detected violations set PatchSet to Rejected and trigger Retry."
            }
          }
        }
      }
    },

    "ToolRule": {
      "type": "object",
      "required": ["tool", "actions"],
      "additionalProperties": false,
      "properties": {
        "tool": {
          "type": "string", "minLength": 1, "maxLength": 128,
          "description": "Tool name, matching Libra ToolInvocation.tool_name. Common tools: workspace.fs, workspace.command, workspace.lsp, workspace.search, web.search."
        },
        "actions": {
          "type": "array", "minItems": 1,
          "items": { "type": "string", "maxLength": 64 },
          "description": "Permitted actions. workspace.fs: read, write, delete; workspace.command: execute; workspace.lsp: hover, goto-definition, find-references."
        },
        "constraints": {
          "type": "object", "additionalProperties": true, "default": {},
          "description": "Tool-specific constraint object. Common keys: writeRoots (permitted write paths), allowCommands (command allowlist), denySubstrings (forbidden command substrings), maxOutputBytes."
        }
      }
    },

    "ExecutionPolicy": {
      "type": "object",
      "description": "Execution strategy: retry, replan triggers, and concurrency. These parameters directly affect the number of Libra Task.runs and Plan revision chain length.",
      "required": ["retry", "replan", "concurrency"],
      "additionalProperties": false,
      "properties": {
        "retry": {
          "type": "object", "required": ["maxRetries"], "additionalProperties": false,
          "properties": {
            "maxRetries": {
              "type": "integer", "minimum": 0, "maximum": 20, "default": 3,
              "description": "Maximum retries per Task (new Run count limit). 0=no retry; 3 (default)=up to 4 Run attempts. High-risk tasks should use lower values (2) to avoid accumulating unnecessary costs."
            },
            "backoffSeconds": {
              "type": "integer", "minimum": 0, "maximum": 3600, "default": 10,
              "description": "Retry wait time base (seconds). Actual wait = backoffSeconds × 2^(retry_count-1), capped at 3600 s."
            }
          }
        },
        "replan": {
          "type": "object", "additionalProperties": false, "default": {},
          "description": "Replan trigger conditions, mapping to Libra Plan.new_revision() call timing.",
          "properties": {
            "triggers": {
              "type": "array",
              "items": { "type": "string",
                         "enum": ["evidence-conflict", "scope-creep", "repeated-test-fail",
                                  "security-gate-fail", "unknown-api"] },
              "default": ["repeated-test-fail", "security-gate-fail", "evidence-conflict"],
              "description": "Conditions that trigger a Plan revision. evidence-conflict=contradictory evidence sources; scope-creep=agent attempts to modify outOfScope files; repeated-test-fail=same test fails consecutively N times; security-gate-fail=security check fails but is potentially addressable; unknown-api=agent calls an API not declared in the IntentSpec."
            }
          }
        },
        "concurrency": {
          "type": "object", "additionalProperties": false, "default": {},
          "properties": {
            "maxParallelTasks": {
              "type": "integer", "minimum": 1, "maximum": 128, "default": 4,
              "description": "Maximum number of concurrently Running Tasks. High-risk tasks should use 1 (serial execution for easier auditing). Actual concurrency is also limited by write-conflict detection results."
            }
          }
        }
      }
    },

    "Artifacts": {
      "type": "object",
      "description": "Required artifact manifest. The Scheduler checks for valid ArtifactRef entries in Evidence.report_artifacts at each gate stage. Any missing required artifact is a gate fail.",
      "required": ["required"],
      "additionalProperties": false,
      "properties": {
        "required": {
          "type": "array", "minItems": 1,
          "items": { "$ref": "#/$defs/ArtifactReq" },
          "description": "Required artifact list. Minimum: patchset and test-log. High-assurance scenarios should include sast-report, sca-report, sbom, provenance-attestation, and transparency-proof."
        },
        "retention": {
          "type": "object", "additionalProperties": false, "default": {},
          "properties": {
            "days": {
              "type": "integer", "minimum": 0, "maximum": 3650, "default": 90,
              "description": "Artifact retention days from Decision.Commit. 0=do not retain; 90 (default); 365=compliance scenarios. The lower of this and constraints.privacy.retentionDays is used."
            }
          }
        }
      }
    },

    "ArtifactReq": {
      "type": "object",
      "required": ["name", "stage", "required"],
      "additionalProperties": false,
      "properties": {
        "name": {
          "type": "string",
          "enum": ["patchset","test-log","build-log","sast-report","sca-report",
                   "sbom","provenance-attestation","transparency-proof","release-notes"],
          "description": "Artifact type. patchset=code diff; test-log=test execution log; build-log=build log; sast-report=SAST scan report (SARIF); sca-report=dependency vulnerability report; sbom=Software Bill of Materials; provenance-attestation=SLSA provenance (in-toto); transparency-proof=Rekor inclusion proof; release-notes=release notes."
        },
        "stage": {
          "type": "string", "enum": ["per-task", "integration", "security", "release"],
          "description": "Gate stage at which this artifact's existence is checked. per-task=after each implementation Task; integration=after integration checks; security=after security checks; release=final gate before Decision.Commit."
        },
        "required": { "type": "boolean", "default": true },
        "format": {
          "type": "string", "maxLength": 64, "default": "",
          "description": "Artifact format identifier: 'git-diff', 'junit+xml', 'sarif', 'spdx-json', 'cyclonedx-json', 'in-toto+json', 'rekor-inclusion-proof', 'markdown', 'text'. Written to ArtifactRef.content_type. Open set: additional formats may be used by convention."
        }
      }
    },

    "ProvenancePolicy": {
      "type": "object",
      "description": "Provenance binding policy. The goal is to make the IntentSpec a verifiable input parameter in the supply-chain evidence chain, strongly bound to the output attestation. Consumers can verify the IntentSpec digest in provenance to confirm 'artifact built from this specific IntentSpec'.",
      "required": ["requireSlsaProvenance", "requireSbom", "transparencyLog", "bindings"],
      "additionalProperties": false,
      "properties": {
        "requireSlsaProvenance": {
          "type": "boolean", "default": true,
          "description": "true = the Scheduler must generate an in-toto SLSA attestation after Decision.Commit. The attestation's externalParameters include intentspec_digest; internalParameters include the Scheduler version."
        },
        "requireSbom": {
          "type": "boolean", "default": true,
          "description": "true = the securityChecks stage must produce an sbom ArtifactRef (SPDX JSON recommended). Implements NIST SSDF PS.3.2 'collect, maintain, share provenance data such as SBOMs'."
        },
        "transparencyLog": {
          "type": "object", "required": ["mode"], "additionalProperties": false,
          "properties": {
            "mode": {
              "type": "string", "enum": ["none", "rekor", "internal-ledger"], "default": "rekor",
              "description": "Transparency log mode. none=no log; rekor=Sigstore Rekor public transparency log (recommended for open-source); internal-ledger=private enterprise log. Using rekor: after Decision.Commit the Scheduler submits the attestation to Rekor and writes the inclusion proof to the transparency-proof ArtifactRef."
            }
          }
        },
        "bindings": {
          "type": "object", "required": ["embedIntentSpecDigest"], "additionalProperties": false,
          "properties": {
            "embedIntentSpecDigest": {
              "type": "boolean", "default": true,
              "description": "true = embed the sha256 digest of the canonical IntentSpec JSON in Provenance.parameters[\"externalParameters\"][\"intentspec_digest\"]. Enables consumers to verify IntentSpec integrity and achieve intent–artifact strong binding."
            },
            "embedEvidenceDigests": {
              "type": "boolean", "default": true,
              "description": "true = embed digests of all Evidence.report_artifacts in Provenance.parameters[\"byproducts\"], extending the provenance chain to every test report and scan report."
            }
          }
        }
      }
    },

    "Lifecycle": {
      "type": "object",
      "description": "IntentSpec state machine and change history. status maps to Libra Intent.status; changeLog is the immutable replan audit log, mapping to Libra Intent.statuses (append-only).",
      "required": ["schemaVersion", "status", "changeLog"],
      "additionalProperties": false,
      "properties": {
        "schemaVersion": {
          "type": "string",
          "pattern": "^[0-9]+\\.[0-9]+\\.[0-9]+$",
          "default": "1.0.0",
          "description": "Semantic version of the IntentSpec schema. Decoupled from apiVersion: apiVersion handles routing (bump only on breaking change); schemaVersion handles field additions or relaxed constraints (backwards-compatible)."
        },
        "status": {
          "type": "string", "enum": ["draft", "active", "deprecated", "closed"], "default": "active",
          "description": "IntentSpec status. draft=being edited; active=executing (Libra Intent.Active); deprecated=superseded; closed=execution complete or cancelled (Intent.Completed/Cancelled). Schedulers only accept active IntentSpecs."
        },
        "changeLog": {
          "type": "array",
          "items": { "$ref": "#/$defs/ChangeLogEntry" },
          "default": [],
          "description": "Append-only change history. The Scheduler appends one ChangeLogEntry per replan event, simultaneously writing to Libra Intent.statuses. Forms the complete decision chain from initial intent to final commit."
        }
      }
    },

    "ChangeLogEntry": {
      "type": "object",
      "required": ["at", "by", "reason", "diffSummary"],
      "additionalProperties": false,
      "properties": {
        "at":          { "type": "string", "format": "date-time" },
        "by":          { "type": "string", "minLength": 1, "maxLength": 128 },
        "reason":      { "type": "string", "minLength": 1, "maxLength": 2000,
                         "description": "Replan reason — trigger name and specific description." },
        "diffSummary": { "type": "string", "minLength": 1, "maxLength": 4000,
                         "description": "Summary of changes relative to the previous version." }
      }
    },

    "LibraBinding": {
      "type": "object",
      "description": "Libra AI Object Model binding configuration. Controls AI object storage backend, ContextPipeline frame management, Plan generation strategy, Run execution detail, and Decision policy. All sub-fields have defaults when the libra field is omitted.",
      "additionalProperties": false,
      "properties": {
        "objectStore": {
          "type": "object", "additionalProperties": false, "default": {},
          "description": "AI object storage configuration.",
          "properties": {
            "backend": {
              "type": "string", "enum": ["git-native", "external-s3", "external-local"], "default": "git-native",
              "description": "AI object storage backend. git-native=store in git object DB under refs/ai/* (Libra native, best for open-source); external-s3=store in S3-compatible storage, git keeps only ArtifactRef (best for large artifacts); external-local=local filesystem (dev/test only)."
            },
            "blobRetentionStrategy": {
              "type": "string",
              "enum": ["ref-anchoring", "orphan-commit", "keep-pack", "custom-gc"],
              "default": "ref-anchoring",
              "description": "GC retention strategy for non-File ContextItem blobs (Url/Snippet/Command). ref-anchoring=create refs/ai/blobs/<hex> to prevent gc (simple, reliable); orphan-commit=store in orphan commit tree; keep-pack=create .keep marker; custom-gc=custom GC scan hook."
            },
            "aiRefPrefix": {
              "type": "string", "default": "refs/ai/",
              "description": "AI object ref namespace prefix. Intents stored as refs/ai/intents/<id>, Plans as refs/ai/plans/<id>, etc."
            }
          }
        },
        "contextPipeline": {
          "type": "object", "additionalProperties": false, "default": {},
          "description": "ContextPipeline creation and frame management policy. max_frames is both a memory control and a security control: more frames means a larger prompt injection accumulation surface.",
          "properties": {
            "maxFrames": {
              "type": "integer", "minimum": 0, "default": 128,
              "description": "Maximum pipeline frames; oldest non-protected frames are evicted when exceeded. 0=unlimited (not recommended). Recommended formula: min(128, maxWallClockSeconds/300). IntentAnalysis and Checkpoint frames are protected and never evicted."
            },
            "seedFrameKind": {
              "type": "string", "enum": ["IntentAnalysis", "Checkpoint"], "default": "IntentAnalysis",
              "description": "Type of the pipeline seed frame (first frame, always protected). IntentAnalysis=recommended for normal flows; Checkpoint=for resumption from an interrupted state."
            },
            "checkpointOnReplan": {
              "type": "boolean", "default": true,
              "description": "true = automatically push a Checkpoint frame (protected) before each Plan.new_revision() call, ensuring each replan origin is traceable and recoverable."
            }
          }
        },
        "planGeneration": {
          "type": "object", "additionalProperties": false, "default": {},
          "description": "Plan and Task DAG generation strategy.",
          "properties": {
            "decompositionMode": {
              "type": "string", "enum": ["per-objective", "per-file-cluster", "single-task"], "default": "per-objective",
              "description": "Task decomposition mode. per-objective=one child Task per intent.objective (recommended, clear boundaries); per-file-cluster=cluster by file dependency graph (large refactors); single-task=all objectives in one Task (simple single-file changes only)."
            },
            "conflictResolution": {
              "type": "string", "enum": ["merge-tasks", "force-serial", "fail-fast"], "default": "force-serial",
              "description": "Write-conflict resolution strategy (when two child Tasks have overlapping io_footprint writes). merge-tasks=merge into one Task; force-serial=keep separate but enforce serial order via Task.dependencies; fail-fast=treat as planning error and immediately replan."
            },
            "gateTaskPerStage": {
              "type": "boolean", "default": true,
              "description": "true = generate a dedicated gate Task for each verification stage (fast/integration/security/release); they execute serially and each blocks the next on failure."
            }
          }
        },
        "runPolicy": {
          "type": "object", "additionalProperties": false, "default": {},
          "description": "Run execution detail, complementing the execution field with Libra-specific configuration.",
          "properties": {
            "patchsetFormat": {
              "type": "string", "enum": ["Unified", "GitDiff"], "default": "GitDiff",
              "description": "PatchSet diff format. GitDiff=standard git diff (recommended, matches Libra DiffFormat::GitDiff); Unified=POSIX unified diff."
            },
            "snapshotOnRunStart": {
              "type": "boolean", "default": true,
              "description": "true = create a ContextSnapshot at the start of each Run (static snapshot used for SLSA provenance resolvedDependencies)."
            },
            "metricsSchema": {
              "type": "object", "additionalProperties": true, "default": {},
              "description": "Expected structure for Run.metrics (free JSON Schema fragment). Common fields: wall_clock_seconds, cost_usd, tokens_used, patchsets_generated."
            }
          }
        },
        "actorMapping": {
          "type": "object", "additionalProperties": false, "default": {},
          "description": "Mapping from IntentSpec roles to Libra ActorRef identifiers. `orchestratorActorId` is a legacy compatibility field name that maps to the Scheduler actor.",
          "properties": {
            "orchestratorActorId": { "type": "string", "default": "libra-orchestrator" },
            "coderActorId":        { "type": "string", "default": "libra-coder" },
            "reviewerActorId":     { "type": "string", "default": "libra-reviewer" }
          }
        },
        "decisionPolicy": {
          "type": "object", "additionalProperties": false, "default": {},
          "description": "Decision type policy for specific situations.",
          "properties": {
            "abandonOnSecurityGateFail": {
              "type": "boolean", "default": true,
              "description": "true = when securityChecks fail, immediately Decision.Abandon rather than Retry (security issues typically require human intervention, not automatic retry)."
            },
            "checkpointBeforeReplan": {
              "type": "boolean", "default": true,
              "description": "true = create a Decision.Checkpoint to save current state before triggering a Plan revision."
            },
            "rollbackOnProvenanceFail": {
              "type": "boolean", "default": true,
              "description": "true = if Provenance generation or Rekor submission fails, Decision.Rollback any already-applied PatchSet. Ensures the dangerous 'code committed but no verifiable provenance' state never reaches the main branch."
            }
          }
        }
      }
    }
  }
}
```

---

## 3. 字段参考

### 3.1 Metadata

`metadata` 是不可变（immutable）的审计锚点。所有字段在创建时设定，且不得修改——修改即意味着创建一个新的 IntentSpec。

**`metadata.id`** 存放全局唯一标识符。调度器将其写入 `Libra Intent.external_ids["intentspec_id"]`。当 `provenance.bindings.embedIntentSpecDigest=true` 时，调度器会冻结规范化 JSON（键排序、无空白），计算 SHA-256 摘要，并将 `id` 与该摘要一并嵌入 `Provenance.parameters.externalParameters`。这实现了**意图—产物的强绑定**：消费方可以从 IntentSpec 文件重新推导出摘要，并与 Provenance 比对，以验证没有被篡改。

**`metadata.createdBy.type`** 决定使用哪个 Libra `ActorRef` 工厂（`human`、`agent`、`system`）。高风险的 IntentSpec 应当源自 `user`，或源自已获得人工批准的 `agent`。

**`metadata.target.baseRef`** 由调度器解析为一个具体的 commit SHA，写入 `Libra Run.commit`。使用完整 SHA（而非浮动的分支名）可为 SLSA 的 `resolvedDependencies` 提供更高的溯源（provenance）保真度。

---

### 3.2 Intent

`intent` 是最重要的部分：它定义了调度器在整个执行过程中所强制执行的工作边界。

**`intent.objectives[]`** 与 Libra 的 `PlanStep` 条目及其子 `Task` 对象一一对应。每个 objective 都应表达一个可独立观察的成功状态。调度器通过监控 `ToolInvocation.io_footprint.paths_written` 与 `inScope` 的关系来检测范围蔓延（scope-creep）；任何越界写入都会触发 `scope-creep` 重规划或直接拒绝。

**`intent.touchHints`** 提供定位信号：`files`（glob 模式）会成为 `ContextItem(File)` 条目；`symbols` 通过 ctags/LSP 解析为 `ContextItem(Snippet)` 帧；`apis` 则在 OpenAPI 规范中查找为 `ContextItem(Url)` 帧。调度器利用 import/build 图，将初始匹配扩展到一跳（one-hop）依赖半径，以避免遗漏改动。

---

### 3.3 Acceptance

`acceptance` 把“成功”转化为客观、可执行的标准。

**`verificationPlan`** 有四个顺序阶段。当 `libra.planGeneration.gateTaskPerStage=true` 时，调度器为每个阶段生成一个专门的门禁（gate）`Task`。阶段顺序是严格的：`fastChecks` 失败会阻止 `integrationChecks` 启动。这既保持了紧凑的反馈循环（单元测试以秒计），又确保昂贵的扫描只在已验证的代码上运行。

**`qualityGates.requireNewTestsWhenBugfix`** 是一项策略级约束（constraints），调度器通过对 PatchSet 做 diff 分析来强制执行：若没有任何测试文件发生变更，则门禁失败。这可防止“修了代码、跳过测试”的反模式。

---

### 3.4 Constraints

`constraints` 编码了四类硬性边界：安全/隐私/许可/资源。

**`constraints.security.networkPolicy=deny`** 是默认值，并直接映射到 ToolInvocation 的预检 ACL：调度器会拒绝任何会发起对外网络联系的工具调用，除非 `security.toolAcl.allow` 中包含一条已显式列入白名单、并附明确原因的命令。这就是“最小网络足迹”基线（与 SLSA 的隔离构建要求一致）。

**`constraints.security.dependencyPolicy`** 驱动 SCA 门禁行为：`no-new` 意味着扫描 SCA 报告以查找任何新引入的包；任何新包都会导致门禁失败。`allow-with-review` 允许新增，但要求产出一个 `sca-report` 产物供人工审查，并对照 `allowedSpdx` 做许可检查。

**`constraints.resources`** 字段在运行时被强制执行——而不仅仅是记录。`maxWallClockSeconds` 设置 Run 超时；`maxCostUnits` 为 `Provenance.token_usage.cost_usd` 的累计设上限。当接近成本上限时，调度器会先把 `maxParallelTasks` 降为 1，再决定是继续还是建立检查点。这直接应对 OWASP 的“Unbounded Consumption”风险（risk）。

---

### 3.5 Risk

`risk.level` 触发调度器强制执行的规则：

| Level | 由调度器强制执行 |
|---|---|
| `low` | 除 schema 外无特殊约束（constraints） |
| `medium` | 若 `humanInLoop.required=false` 且改动触及安全相关代码，则发出警告 |
| `high` | **要求** `humanInLoop.required=true` 且 `minApprovers>=2`；要求 `releaseChecks` 包含一项 `require-approvers` 策略检查；若 `maxParallelTasks` 未设置则降为 1 |

---

### 3.6 Evidence Policy

`evidence` 是抵御 prompt injection 的第一道防线。

**`strategy=repo-first`** 意味着调度器优先采用目标仓库内部的信息（代码、注释、README、现有测试）。外部网络访问被降到最低。这缩小了外部文档中嵌入恶意内容的攻击面。

**`domainAllowlistMode=allowlist-only`** 配合 `blockedDomains=["*"]` 是最严格的配置：只有显式列于 `allowedDomains` 的域名才能被访问。求值顺序为：先检查 `allowedDomains`，然后对任何不在白名单内的域名应用 `blockedDomains` 作为减法式拒绝列表（deny-list）。Url 类型的 `ContextItem` 入队前会做预检；被阻断的条目会在不被加入流水线的情况下遭拒。

**`minCitationsPerDecision`** 强制调度器为关键决策（Decision）记录证据（evidence）溯源（provenance）。当设为 3 时，调度器在 ContextPipeline 中没有至少 3 个支撑来源的情况下，拒绝选定某个算法或库，从而形成一条可审计的证据链。

---

### 3.7 Security Policy

**`security.toolAcl`** 实现了“最小权限闭包”原则。每个 `ToolRule` 上的 `constraints` 字段携带工具专属限制：文件系统工具的 `writeRoots`，命令类工具的 `allowCommands` / `denySubstrings`，以及任意工具的 `maxOutputBytes`。调度器在创建每条 `ToolInvocation` 记录之前都会检查这些约束（constraints）。

**`security.secrets.policy=deny-all`**（默认）意味着 Run 执行环境不会被注入任何 secret——这与 SLSA 要求签名材料不得对用户构建步骤可见的规定一致。

**`security.outputHandling.noDirectEval=true`** 会在每个 PatchSet diff 转入 `Proposed` 之前触发 AST 级扫描。该扫描使用与语言匹配的正则/AST 模式来检测 `eval()`、`exec()`、`subprocess(shell=True)`、`os.system()` 及等价构造。检测到的违规会将 `PatchSet.apply_status=Rejected` 并触发一次 Retry。

---

### 3.8 Execution Policy

**`execution.retry.maxRetries`** 限定单个 Task 的 `Decision.Retry` 事件（Event）次数。耗尽后将创建 `Decision.Abandon`。高风险任务应使用更低的值（2），以便尽快将持续性失败暴露给人。

**`execution.replan.triggers`** 指定调度器调用 `Plan.new_revision()` 的条件。在此之前，调度器（当 `libra.decisionPolicy.checkpointBeforeReplan=true` 时）会创建 `Decision.Checkpoint` 以保留中间进展。旧的 Plan 版本在修订链中保持不可变（immutable），从而可完整重建重规划历史。

---

### 3.9 Artifacts

`artifacts.required[]` 中的每一条都会在某个 `verificationPlan.*Check`（通过 `artifactsProduced`）与门禁（gate）检查器之间建立一份契约。流程如下：

```
Check.command executes
  → ToolInvocation records io_footprint
  → Evidence.report_artifacts[] populated with ArtifactRef (key, hash, expires_at)
  → Gate checker verifies: for each required artifact at this stage,
    Evidence.report_artifacts must contain a matching ArtifactRef with valid hash
  → Missing or hash-invalid artifact → gate fail
```

`artifacts.retention.days` 在创建时写入 `ArtifactRef.expires_at`。取它与 `constraints.privacy.retentionDays` 二者中的较小值。这确保产物生命周期与数据隐私承诺相匹配。

---

### 3.10 Provenance Policy

`provenance` 将 IntentSpec 与 SLSA 供应链证据（evidence）模型连接起来。

当 `requireSlsaProvenance=true` 时，调度器会在 `Decision.Commit` 之后生成一份 DSSE 封装的 in-toto 证明（attestation）。该证明包含：
- `externalParameters.intentspec_id` 与 `externalParameters.intentspec_digest`（来自冻结的规范化 JSON）
- `internalParameters.scheduler_version`
- `resolvedDependencies`：每个 `ContextSnapshot.item` 贡献一条 `{uri, digest}` 条目
- `byproducts`：当 `embedEvidenceDigests=true` 时，所有 `Evidence.report_artifacts` 的摘要

当 `transparencyLog.mode=rekor` 时，签名后的证明会在提交后上传至 Rekor，并将 Rekor 的包含证明（inclusion proof）作为 `transparency-proof` ArtifactRef 写回。若上传失败且 `libra.decisionPolicy.rollbackOnProvenanceFail=true`，调度器会发出 `Decision.Rollback` 以回滚已应用的 PatchSet，确保仓库永远不会包含一个没有可验证溯源（provenance）的提交。

---

### 3.11 Lifecycle

`lifecycle.changeLog` 是一份关于所有重规划事件（Event）的追加式（append-only）记录。每条记录捕获：

- `at`：重规划被触发的时间
- `by`：行为者（调度器 ID 或人工审批者）
- `reason`：触发条件
- `diffSummary`：相对上一版本，IntentSpec 中发生了什么变化

这份日志与 `Libra Intent.statuses` 结合，提供了一条完整的审计链路——从最初的用户意图，经历每一次规划修订，直至最终提交的结果。

---

### 3.12 Libra Binding

`libra` 是一个可选的 Libra 专属配置块。所有子字段均有默认值。

**`libra.contextPipeline.maxFrames`** 是一项双重用途的控制项：它同时限制内存占用与 prompt injection 的累积面。推荐公式为 `min(128, maxWallClockSeconds / 300)`——约每 5 分钟执行对应一帧。`IntentAnalysis` 与 `Checkpoint` 帧始终受保护，不会被驱逐。

**`libra.decisionPolicy.rollbackOnProvenanceFail=true`** 弥合了一个关键缺口：若没有它，一次 Rekor 提交失败会留下一个没有透明日志条目的 git commit。有了它，PatchSet 会被回滚，并由调度器发出失败信号以待人工介入。

**`libra.actorMapping`** 允许你为安全敏感的改动使用专门的 agent ID。例如，`coderActorId=libra-security-coder` 与 `reviewerActorId=libra-security-reviewer` 使平台能够路由到经过安全专项训练、且工具限制更严格的 agent。

---

## 4. 字段速查表

| Field Path | Type | Required | Default | Key Role | Related Fields |
|---|---|---|---|---|---|
| `apiVersion` | string | ✅ | `intentspec.io/v1alpha1` | 调度器兼容性路由 | `lifecycle.schemaVersion` |
| `kind` | string(const) | ✅ | `IntentSpec` | 资源类型 | — |
| `metadata.id` | string | ✅ | — | 全局唯一 ID → Libra external_ids | `provenance.bindings.embedIntentSpecDigest` |
| `metadata.createdAt` | date-time | ✅ | — | 溯源（provenance）时间锚点 | — |
| `metadata.createdBy` | object | ✅ | — | Libra ActorRef 映射 | `risk.level` |
| `metadata.target.repo` | object | ✅ | — | git clone 目标 | `execution.*` |
| `metadata.target.baseRef` | string | ✅ | — | Run.commit 基线 | `provenance.*` |
| `intent.summary` | string | ✅ | — | Task.title + PR 标题 | — |
| `intent.problemStatement` | string | ✅ | — | Intent.prompt（不可变） | — |
| `intent.changeType` | enum | ✅ | `unknown` | Task.goal，影响 qualityGates | `acceptance.qualityGates` |
| `intent.objectives[]` | array | ✅ | — | 生成子 Task 与 PlanStep | `libra.planGeneration.decompositionMode` |
| `intent.inScope[]` | array | ✅ | — | Task.constraints，ACL 检查 | `security.toolAcl` |
| `intent.outOfScope[]` | array | — | `[]` | Task.constraints，范围蔓延（scope-creep）检测 | `execution.replan.triggers` |
| `intent.touchHints` | object | — | `{}` | ContextSnapshot.items 生成 | `evidence.strategy` |
| `acceptance.successCriteria[]` | array | ✅ | — | Task.acceptance_criteria | — |
| `acceptance.verificationPlan` | object | ✅ | — | 四阶段门禁（gate）Task | `artifacts.required` |
| `acceptance.qualityGates` | object | — | `{}` | 元策略约束（constraints） | `intent.changeType` |
| `constraints.security.networkPolicy` | enum | ✅ | `deny` | ToolInvocation 预检 ACL | `security.toolAcl` |
| `constraints.security.dependencyPolicy` | enum | ✅ | `allow-with-review` | SCA 门禁触发 | `artifacts.required` |
| `constraints.privacy.dataClassesAllowed[]` | array | ✅ | `[public]` | ContextItem 过滤 | — |
| `constraints.licensing.allowedSpdx[]` | array | ✅ | `[]` | SCA 许可检查 | `artifacts.required` |
| `constraints.resources.maxWallClockSeconds` | integer | — | `14400` | Run 超时 + max_frames 推导 | `libra.contextPipeline.maxFrames` |
| `risk.level` | enum | ✅ | `medium` | 强制门禁（gate）配置 | `risk.humanInLoop` |
| `risk.humanInLoop.required` | boolean | ✅ | `false` | releaseChecks require-approvers | `risk.level` |
| `evidence.strategy` | enum | ✅ | `repo-first` | ContextSnapshot.selection_strategy | `evidence.domainAllowlistMode` |
| `evidence.domainAllowlistMode` | enum | ✅ | `allowlist-only` | URL ContextItem 域名检查 | `evidence.allowedDomains` |
| `security.toolAcl.allow[]` | array | ✅ | — | ToolInvocation 预检拦截 | `constraints.security.networkPolicy` |
| `security.secrets.policy` | enum | ✅ | `deny-all` | Run 环境 secret 注入控制 | — |
| `security.promptInjection.*` | object | ✅ | — | ContextFrame 可信度（trust）标记 | `evidence.strategy` |
| `security.outputHandling.noDirectEval` | boolean | ✅ | `true` | PatchSet AST 扫描 | — |
| `execution.retry.maxRetries` | integer | ✅ | `3` | Decision.Retry 次数上限 | `execution.replan.triggers` |
| `execution.replan.triggers[]` | array | — | `[...]` | Plan.new_revision() 触发 | `libra.decisionPolicy` |
| `execution.concurrency.maxParallelTasks` | integer | — | `4` | 并行 Task 数量上限 | `constraints.resources` |
| `artifacts.required[]` | array | ✅ | — | Evidence ArtifactRef 检查 | `acceptance.verificationPlan` |
| `artifacts.retention.days` | integer | — | `90` | ArtifactRef.expires_at | `constraints.privacy.retentionDays` |
| `provenance.requireSlsaProvenance` | boolean | ✅ | `true` | Provenance 对象创建 | `provenance.bindings` |
| `provenance.requireSbom` | boolean | ✅ | `true` | securityChecks sbom 检查 | `artifacts.required` |
| `provenance.transparencyLog.mode` | enum | ✅ | `rekor` | Rekor 提交 + transparency-proof | `libra.decisionPolicy.rollbackOnProvenanceFail` |
| `provenance.bindings.embedIntentSpecDigest` | boolean | ✅ | `true` | Provenance.externalParameters | — |
| `lifecycle.status` | enum | ✅ | `active` | Intent.status 映射 | — |
| `lifecycle.changeLog[]` | array | ✅ | `[]` | 重规划审计日志 | `execution.replan.triggers` |
| `libra.objectStore` | object | — | defaults | git object 存储配置 | — |
| `libra.contextPipeline` | object | — | defaults | ContextPipeline 帧管理 | `constraints.resources.maxWallClockSeconds` |
| `libra.planGeneration` | object | — | defaults | Task DAG 生成策略 | `intent.objectives` |
| `libra.runPolicy` | object | — | defaults | Run 执行细节 | — |
| `libra.actorMapping` | object | — | defaults | Libra ActorRef 映射 | `metadata.createdBy` |
| `libra.decisionPolicy` | object | — | defaults | Decision 类型策略 | `risk.level` |

---

## 5. 示例 1 —— 最小化（低风险 bugfix）

**场景：** 某 TypeScript 服务中的空指针解引用导致 HTTP 500 错误。仅影响单个文件的输入校验逻辑；无新增依赖，无 API 结构变更。

**关键参数选择：**
- `risk.level = low` —— 无需人工批准
- `constraints.security.dependencyPolicy = no-new` —— 不引入依赖
- `provenance.requireSbom = false` —— 该风险级别不要求 SBOM
- `provenance.transparencyLog.mode = none` —— 无透明日志
- `execution.concurrency.maxParallelTasks = 2` —— 两个 objective，可并行运行
- `libra.contextPipeline.maxFrames = 32` —— 小任务用小上下文窗口

参见文件：
- [`intentspec_minimal.json`](intentspec_minimal.json) —— 机器可读，用于测试与校验
- [`intentspec_minimal.yaml`](intentspec_minimal.yaml) —— 人类可读的 YAML 表示

---

## 6. 示例 2 —— 典型（中风险新功能）

**场景：** 某 Python API 服务为 `GET /v2/report` 新增一个可选的 `fields` 查询参数，用于字段白名单过滤。涉及多个文件；需要契约测试与安全扫描。

**关键参数选择：**
- `risk.level = medium` —— 需要 1 名审批者
- `constraints.security.dependencyPolicy = allow-with-review` —— 允许新增依赖但需要 SCA
- `provenance.requireSbom = true` —— 需要 SBOM
- `provenance.transparencyLog.mode = rekor` —— 接入 Rekor
- `evidence.strategy = pinned-official` —— 允许白名单内的官方文档
- `libra.contextPipeline.maxFrames = 64` —— 中等复杂度

参见文件：
- [`intentspec_typical.json`](intentspec_typical.json)
- [`intentspec_typical.yaml`](intentspec_typical.yaml)

---

## 7. 示例 3 —— 高保障（高风险安全修复）

**场景：** 某鉴权中间件存在条件性 JWT 绕过漏洞（CVE）。修复需要完整可审计性：SBOM + SLSA 溯源（provenance） + Rekor 包含证明（inclusion proof），并由安全团队执行四眼（four-eyes）批准。

**关键参数选择：**
- `risk.level = high` —— 强制 `humanInLoop.minApprovers = 2`
- `constraints.security.dependencyPolicy = no-new` —— 安全修复不引入新依赖
- `execution.concurrency.maxParallelTasks = 1` —— 串行执行，便于审计
- `libra.contextPipeline.maxFrames = 64` —— 控制信息暴露面
- `libra.decisionPolicy.rollbackOnProvenanceFail = true` —— 溯源（provenance）失败时回滚
- `evidence.blockedDomains = ["*"]` —— 与 `allowedDomains` 结合形成严格白名单

参见文件：
- [`intentspec_high_assurance.json`](intentspec_high_assurance.json)
- [`intentspec_high_assurance.yaml`](intentspec_high_assurance.yaml)

---

## 8. 示例参数对比

| Parameter | Minimal (low-risk) | Typical (medium-risk) | High-assurance (high-risk) |
|---|---|---|---|
| `risk.level` | `low` | `medium` | `high` |
| `humanInLoop.minApprovers` | `0` | `1` | `2` |
| `dependencyPolicy` | `no-new` | `allow-with-review` | `no-new` |
| `evidence.strategy` | `repo-first` | `pinned-official` | `repo-first` |
| `evidence.minCitationsPerDecision` | `0` | `2` | `3` |
| `maxParallelTasks` | `2` | `4` | `1` |
| `maxRetries` | `3` | `3` | `2` |
| `requireSlsaProvenance` | `false` | `true` | `true` |
| `requireSbom` | `false` | `true` | `true` |
| `transparencyLog.mode` | `none` | `rekor` | `rekor` |
| `artifacts.retention.days` | `7` | `180` | `365` |
| `contextPipeline.maxFrames` | `32` | `64` | `64` |
| `gateTaskPerStage` | `false` | `true` | `true` |
| `checkpointOnReplan` | `false` | `true` | `true` |
| `rollbackOnProvenanceFail` | `false` | `true` | `true` |
| `blockedDomains` | `[]` | `[]` | `["*"]` |
| securityChecks 数量 | `0` | `2` | `4` |
| releaseChecks 数量 | `0` | `3` | `4` |
| 必需产物数量 | `2` | `7` | `8` |
