# IntentSpec Design

**Version:** 1.0.0  
**Spec foundations:** JSON Schema Draft 2020-12 · NIST SSDF · SLSA v1.0 · OWASP LLM Top 10 (2025)  
**Execution layer:** Libra AI Object Model (`git-internal`)

---

## Table of Contents

1. [Design Philosophy & Architecture Layers](#1-design-philosophy--architecture-layers)
2. [Complete JSON Schema (with Libra Extension)](#2-complete-json-schema-with-libra-extension)
3. [Field Reference](#3-field-reference)
4. [Field Quick-Reference Table](#4-field-quick-reference-table)
5. [Example 1 — Minimal (low-risk bugfix)](#5-example-1--minimal-low-risk-bugfix)
6. [Example 2 — Typical (medium-risk new feature)](#6-example-2--typical-medium-risk-new-feature)
7. [Example 3 — High-assurance (high-risk security fix)](#7-example-3--high-assurance-high-risk-security-fix)
8. [Example Parameter Comparison](#8-example-parameter-comparison)

---

## 1. Design Philosophy & Architecture Layers

IntentSpec is a **machine-readable intent contract**. It transforms a natural-language request into a structured, verifiable input that an orchestrator can schedule, gate, and audit. It is not a prompt — it is a contract carrying:

- **Intent** — what to do, what not to do, and what an acceptable outcome looks like
- **Constraints** — hard boundaries around security, privacy, licensing, and resources
- **Gates** — the checks that must pass before each pipeline stage advances
- **Evidence policy** — where to source information and how much to trust each source
- **Provenance bindings** — how to cryptographically link intent, execution, and final artifacts

Within the Libra system IntentSpec occupies the **control plane**; the Libra AI Object Model occupies the **execution plane**:

```
IntentSpec  (control plane)
     │  drives
     ▼
Libra: Intent → Plan → Task DAG → Run → PatchSet → Evidence → Decision
     │  produces
     ▼
git commit + SBOM + attestation + Rekor proof
```

### Standard Alignment

| Standard | How IntentSpec uses it |
|---|---|
| **NIST SSDF** | `artifacts.required` and `provenance.*` implement PS.3.2 (collect/maintain/share provenance data such as SBOM) |
| **SLSA v1.0** | `provenance.bindings.embedIntentSpecDigest` places IntentSpec as an `externalParameter`; `transparencyLog.mode=rekor` satisfies the transparency log requirement |
| **OWASP LLM Top 10 (2025)** | `security.toolAcl` → Excessive Agency; `evidence.domainAllowlistMode` → Prompt Injection; `security.outputHandling` → Improper Output Handling; `constraints.resources` → Unbounded Consumption |

---

## 2. Complete JSON Schema (with Libra Extension)

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "urn:libra:intentspec:v1",
  "title": "IntentSpec (Libra Edition)",
  "description": "Machine-readable AI code-change intent contract. Drives the orchestrator to produce a Task DAG, execute verification gates, and bind provenance artifacts.",
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
      "description": "IntentSpec API version, controls orchestrator compatibility routing. Bump the major version on breaking changes (v1→v2). Decoupled from lifecycle.schemaVersion: apiVersion routes, schemaVersion governs field evolution.",
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
          "description": "Target repository and baseline. The orchestrator clones via repo.locator and resolves baseRef to a commit SHA written to Libra Run.commit.",
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
      "description": "Structured expression of user intent. The orchestrator derives the Task DAG from this field and enforces scope-creep detection throughout execution. Maps to Libra Intent.prompt/content.",
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
          "description": "Scope of allowed changes. Written to Libra Task.constraints (prefix 'in-scope:'). The orchestrator checks ToolInvocation io_footprint.paths_written against this list; violations trigger scope-creep replan or rejection.",
          "items": { "type": "string", "minLength": 1, "maxLength": 2000 }
        },
        "outOfScope": {
          "type": "array", "default": [],
          "description": "Explicitly disallowed areas. Written to Task.constraints (prefix 'out-of-scope:'). Prevents the agent from opportunistically modifying adjacent code.",
          "items": { "type": "string", "minLength": 1, "maxLength": 2000 }
        },
        "touchHints": {
          "type": "object",
          "description": "Hints for touch-point localisation. files/symbols/apis are used by the orchestrator to perform static repository searches (ripgrep/ctags/LSP) and generate Libra ContextSnapshot.items[].",
          "additionalProperties": false, "default": {},
          "properties": {
            "files":   { "type": "array", "default": [],
                         "description": "File glob patterns (e.g. 'src/auth/**'). Matched files become ContextItem(File) entries; their blob hashes are used for SLSA resolvedDependencies.",
                         "items": { "type": "string", "minLength": 1, "maxLength": 512 } },
            "symbols": { "type": "array", "default": [],
                         "description": "Code symbol names (functions, classes, methods). The orchestrator locates definitions/references via ctags/LSP and creates ContextItem(Snippet) frames.",
                         "items": { "type": "string", "minLength": 1, "maxLength": 256 } },
            "apis":    { "type": "array", "default": [],
                         "description": "API endpoint paths (e.g. '/v2/report'). The orchestrator looks up OpenAPI spec files and creates ContextItem(Url) frames, domain-checking against allowedDomains.",
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
                     "description": "Command string. The orchestrator validates against security.toolAcl.allow before creating a ToolInvocation. $ENV_VAR references are resolved by the orchestrator, not read from the IntentSpec." },
        "timeoutSeconds": { "type": "integer", "minimum": 1, "maximum": 86400, "default": 900,
                            "description": "Timeout in seconds. Exceeded → gate fail. Recommended: fastChecks<600, integrationChecks<3600, securityChecks<7200." },
        "expectedExitCode": { "type": "integer", "minimum": 0, "maximum": 255, "default": 0,
                              "description": "Expected process exit code. The orchestrator compares Evidence.exit_code to this value." },
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
              "description": "Network access policy. deny (default) = the orchestrator rejects any ToolInvocation involving external network access (curl, wget, direct npm install, etc.) unless explicitly white-listed in toolAcl with a stated reason."
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
              "description": "Allowed data classification levels during code generation, evidence collection, and logging. The orchestrator filters ContextSnapshot items and redacts any content exceeding these classes."
            },
            "redactionRequired": {
              "type": "boolean", "default": true,
              "description": "true = the orchestrator applies a redaction pipeline before writing any ArtifactRef content."
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
          "description": "Resource budget — both a cost control and a security control against Unbounded Consumption. The orchestrator must actively enforce these fields.",
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
      "description": "Risk classification and human-approval policy. risk.level is not just a label — it drives orchestrator behaviour. The orchestrator validates the consistency of level with humanInLoop in the semantic-validation step.",
      "required": ["level", "rationale", "humanInLoop"],
      "additionalProperties": false,
      "properties": {
        "level": {
          "type": "string", "enum": ["low", "medium", "high"], "default": "medium",
          "description": "Risk level. low=input validation, docs, side-effect-free chores; medium=new features, refactors, data-path changes; high=security fixes, auth/authz changes, crypto, release-blocking defects. Orchestrator rule: high requires humanInLoop.required=true and minApprovers>=2."
        },
        "rationale": {
          "type": "string", "minLength": 5, "maxLength": 4000,
          "description": "Justification for the chosen level. Should cover: impact analysis, potential failure modes, and reasoning for not choosing a higher or lower level. Written to Task.constraints[] as audit context."
        },
        "factors": {
          "type": "array", "default": [],
          "description": "Specific risk factor tags (e.g. [\"authz\", \"cve\", \"release-blocking\"]). Orchestrators may use these to auto-configure additional checks.",
          "items": { "type": "string", "maxLength": 256 }
        },
        "humanInLoop": {
          "type": "object",
          "required": ["required", "minApprovers"],
          "additionalProperties": false,
          "properties": {
            "required": {
              "type": "boolean", "default": false,
              "description": "true = the orchestrator must await a human-approval signal (PR approval, change-order approval, etc.) before Decision.Commit."
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
      "description": "Evidence sourcing and trust policy. Controls where the orchestrator fetches information (repository, official docs, internet) and how much each source is trusted. The first line of defence against prompt injection: restricts external content to a controlled boundary.",
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
          "description": "Allowed evidence trust tiers (descending priority). The orchestrator checks source trust tier when enqueuing ContextItems and tags each with tags[\"trust_tier\"]."
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
          "description": "Tool Access Control List. The orchestrator checks ACL before creating each ToolInvocation: deny rules first (deny takes priority), then allow rules. Any tool call not in the allow list is rejected.",
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
              "description": "true = the orchestrator structurally validates every LLM response (PatchSet format, Evidence format, etc.). Invalid structure is treated as failure and triggers Retry rather than being used as-is."
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
              "description": "true = the orchestrator performs AST-level scanning of all PatchSets to detect and reject eval(), exec(), subprocess(shell=True), os.system(), and equivalent patterns. Detected violations set PatchSet to Rejected and trigger Retry."
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
          "description": "Tool name, matching Libra ToolInvocation.tool_name. Common tools: workspace.fs, workspace.command, workspace.lsp, workspace.search."
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
      "description": "Required artifact manifest. The orchestrator checks for valid ArtifactRef entries in Evidence.report_artifacts at each gate stage. Any missing required artifact is a gate fail.",
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
          "description": "true = the orchestrator must generate an in-toto SLSA attestation after Decision.Commit. The attestation's externalParameters include intentspec_digest; internalParameters include the orchestrator version."
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
              "description": "Transparency log mode. none=no log; rekor=Sigstore Rekor public transparency log (recommended for open-source); internal-ledger=private enterprise log. Using rekor: after Decision.Commit the orchestrator submits the attestation to Rekor and writes the inclusion proof to the transparency-proof ArtifactRef."
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
          "description": "IntentSpec status. draft=being edited; active=executing (Libra Intent.Active); deprecated=superseded; closed=execution complete or cancelled (Intent.Completed/Cancelled). Orchestrators only accept active IntentSpecs."
        },
        "changeLog": {
          "type": "array",
          "items": { "$ref": "#/$defs/ChangeLogEntry" },
          "default": [],
          "description": "Append-only change history. The orchestrator appends one ChangeLogEntry per replan event, simultaneously writing to Libra Intent.statuses. Forms the complete decision chain from initial intent to final commit."
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
          "description": "Mapping from IntentSpec roles to Libra ActorRef identifiers.",
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

## 3. Field Reference

### 3.1 Metadata

`metadata` is the immutable audit anchor. All fields are set at creation time and must not be mutated — modifications mean creating a new IntentSpec.

**`metadata.id`** stores the globally unique identifier. The orchestrator writes it to `Libra Intent.external_ids["intentspec_id"]`. When `provenance.bindings.embedIntentSpecDigest=true`, the orchestrator freezes the canonical JSON (sorted keys, no whitespace), computes a SHA-256 digest, and embeds both the `id` and the digest in `Provenance.parameters.externalParameters`. This achieves **intent–artifact strong binding**: consumers can re-derive the digest from the IntentSpec file and compare it to the Provenance to verify nothing was tampered.

**`metadata.createdBy.type`** governs which Libra `ActorRef` factory is used (`human`, `agent`, `system`). High-risk IntentSpecs should originate from `user` or an `agent` that has received human approval.

**`metadata.target.baseRef`** is resolved by the orchestrator to a concrete commit SHA written to `Libra Run.commit`. Using a full SHA (rather than a floating branch name) gives better provenance fidelity for SLSA `resolvedDependencies`.

---

### 3.2 Intent

`intent` is the most important section: it defines the work boundaries that the orchestrator enforces throughout execution.

**`intent.objectives[]`** maps one-to-one to Libra `PlanStep` entries and child `Task` objects. Each objective should express an independently observable success state. The orchestrator detects scope-creep by monitoring `ToolInvocation.io_footprint.paths_written` against `inScope`; any write outside the boundary triggers `scope-creep` replan or outright rejection.

**`intent.touchHints`** provides localisation signals: `files` (glob patterns) become `ContextItem(File)` entries; `symbols` are resolved via ctags/LSP into `ContextItem(Snippet)` frames; `apis` are looked up in OpenAPI specs as `ContextItem(Url)` frames. The orchestrator expands the initial match to a one-hop dependency radius using the import/build graph to avoid missed changes.

---

### 3.3 Acceptance

`acceptance` converts "success" into objective, executable criteria.

**`verificationPlan`** has four sequential stages. The orchestrator generates a dedicated gate `Task` for each stage when `libra.planGeneration.gateTaskPerStage=true`. Stage ordering is strict: `fastChecks` failures prevent `integrationChecks` from starting. This keeps feedback loops tight (seconds for unit tests) while ensuring expensive scans only run on already-validated code.

**`qualityGates.requireNewTestsWhenBugfix`** is a policy-level constraint the orchestrator enforces by diff-analysing the PatchSet: if no test file has changed, the gate fails. This prevents the anti-pattern of "fix code, skip tests".

---

### 3.4 Constraints

`constraints` encodes four hard boundaries: security/privacy/licensing/resources.

**`constraints.security.networkPolicy=deny`** is the default and maps directly to ToolInvocation pre-flight ACL: the orchestrator rejects any tool call that would make external network contact unless `security.toolAcl.allow` contains an explicit whitelisted command with a stated reason. This is the "minimal network footprint" baseline (aligned with SLSA isolated-build requirements).

**`constraints.security.dependencyPolicy`** drives SCA gate behaviour: `no-new` means the SCA report is scanned for any newly introduced package; any new package is a gate fail. `allow-with-review` permits additions but mandates an `sca-report` artifact for human review and a licence check against `allowedSpdx`.

**`constraints.resources`** fields are enforced at runtime — not just recorded. `maxWallClockSeconds` sets the Run timeout; `maxCostUnits` caps `Provenance.token_usage.cost_usd` accumulation. When the cost cap is approached, the orchestrator reduces `maxParallelTasks` to 1 before deciding whether to continue or checkpoint. This directly addresses the OWASP "Unbounded Consumption" risk.

---

### 3.5 Risk

`risk.level` triggers orchestrator-enforced rules:

| Level | Enforced by orchestrator |
|---|---|
| `low` | No special constraints beyond schema |
| `medium` | Warns if `humanInLoop.required=false` and change touches security code |
| `high` | **Requires** `humanInLoop.required=true` and `minApprovers>=2`; requires `releaseChecks` to include a `require-approvers` policy check; reduces `maxParallelTasks` to 1 if not already set |

---

### 3.6 Evidence Policy

`evidence` is the first line of defence against prompt injection.

**`strategy=repo-first`** means the orchestrator prioritises information from within the target repository (code, comments, README, existing tests). External network access is minimised. This reduces the attack surface for malicious content embedded in external documentation.

**`domainAllowlistMode=allowlist-only`** with `blockedDomains=["*"]` is the strictest configuration: only domains explicitly listed in `allowedDomains` can be accessed. Evaluation order is: check `allowedDomains` first, then apply `blockedDomains` as a subtractive deny-list for any non-allowlisted domains. Url-type `ContextItem` enqueuing is pre-flight checked; blocked items are rejected without being added to the pipeline.

**`minCitationsPerDecision`** forces the orchestrator to log evidence provenance for key decisions. When set to 3, the orchestrator refuses to select an algorithm or library without having at least 3 supporting sources in the ContextPipeline, which creates an auditable evidence chain.

---

### 3.7 Security Policy

**`security.toolAcl`** implements the "minimum privilege closure" principle. The `constraints` field on each `ToolRule` carries tool-specific limits: `writeRoots` for filesystem tools, `allowCommands` / `denySubstrings` for command tools, `maxOutputBytes` for any tool. The orchestrator checks these constraints before creating each `ToolInvocation` record.

**`security.secrets.policy=deny-all`** (default) means the Run execution environment receives no injected secrets — consistent with SLSA's requirement that signing material must not be visible to user build steps.

**`security.outputHandling.noDirectEval=true`** triggers an AST-level scan of every PatchSet diff before it is transitioned to `Proposed`. The scan uses language-appropriate regex/AST patterns to detect `eval()`, `exec()`, `subprocess(shell=True)`, `os.system()` and equivalent constructs. Detected violations set `PatchSet.apply_status=Rejected` and trigger a Retry.

---

### 3.8 Execution Policy

**`execution.retry.maxRetries`** bounds the number of `Decision.Retry` events for a single Task. After exhaustion, `Decision.Abandon` is created. High-risk tasks should use lower values (2) to surface persistent failures to humans quickly.

**`execution.replan.triggers`** specifies the conditions under which the orchestrator calls `Plan.new_revision()`. Before doing so, the orchestrator (when `libra.decisionPolicy.checkpointBeforeReplan=true`) creates `Decision.Checkpoint` to preserve intermediate progress. The old Plan version remains immutable in the revision chain, enabling complete replan history reconstruction.

---

### 3.9 Artifacts

Every entry in `artifacts.required[]` creates a contract between a `verificationPlan.*Check` (via `artifactsProduced`) and the gate checker. The flow is:

```
Check.command executes
  → ToolInvocation records io_footprint
  → Evidence.report_artifacts[] populated with ArtifactRef (key, hash, expires_at)
  → Gate checker verifies: for each required artifact at this stage,
    Evidence.report_artifacts must contain a matching ArtifactRef with valid hash
  → Missing or hash-invalid artifact → gate fail
```

`artifacts.retention.days` is written to `ArtifactRef.expires_at` at creation time. The lower of this value and `constraints.privacy.retentionDays` is used. This ensures artifact lifecycle matches data-privacy commitments.

---

### 3.10 Provenance Policy

`provenance` connects the IntentSpec to the SLSA supply-chain evidence model.

When `requireSlsaProvenance=true`, the orchestrator generates a DSSE-enveloped in-toto attestation after `Decision.Commit`. The attestation contains:
- `externalParameters.intentspec_id` and `externalParameters.intentspec_digest` (from the frozen canonical JSON)
- `internalParameters.orchestrator_version`
- `resolvedDependencies`: each `ContextSnapshot.item` contributes a `{uri, digest}` entry
- `byproducts`: digests of all `Evidence.report_artifacts` when `embedEvidenceDigests=true`

When `transparencyLog.mode=rekor`, the signed attestation is uploaded to Rekor after commit, and the Rekor inclusion proof is written back as the `transparency-proof` ArtifactRef. If the upload fails and `libra.decisionPolicy.rollbackOnProvenanceFail=true`, the orchestrator issues `Decision.Rollback` to revert the applied PatchSet, ensuring the repository never contains a commit without verifiable provenance.

---

### 3.11 Lifecycle

`lifecycle.changeLog` is an append-only record of all replan events. Each entry captures:

- `at`: when the replan was triggered
- `by`: the actor (orchestrator ID or human approver)
- `reason`: the trigger condition
- `diffSummary`: what changed in the IntentSpec relative to the previous version

This log, combined with `Libra Intent.statuses`, provides a complete audit trail from the initial user intent through every planning revision to the final committed result.

---

### 3.12 Libra Binding

`libra` is an optional Libra-specific configuration block. All sub-fields default.

**`libra.contextPipeline.maxFrames`** is a dual-purpose control: it limits both memory usage and prompt injection accumulation surface. The recommended formula is `min(128, maxWallClockSeconds / 300)` — approximately one frame per 5 minutes of execution. `IntentAnalysis` and `Checkpoint` frames are always protected from eviction.

**`libra.decisionPolicy.rollbackOnProvenanceFail=true`** closes a critical gap: without it, a Rekor submission failure would leave a git commit without a transparency log entry. With it, the PatchSet is reverted and the orchestrator signals the failure for human intervention.

**`libra.actorMapping`** allows you to use specialised agent IDs for security-sensitive changes. For example, `coderActorId=libra-security-coder` and `reviewerActorId=libra-security-reviewer` enables the platform to route to agents with security-specific training and stricter tool restrictions.

---

## 4. Field Quick-Reference Table

| Field Path | Type | Required | Default | Key Role | Related Fields |
|---|---|---|---|---|---|
| `apiVersion` | string | ✅ | `intentspec.io/v1alpha1` | Orchestrator compatibility routing | `lifecycle.schemaVersion` |
| `kind` | string(const) | ✅ | `IntentSpec` | Resource type | — |
| `metadata.id` | string | ✅ | — | Global unique ID → Libra external_ids | `provenance.bindings.embedIntentSpecDigest` |
| `metadata.createdAt` | date-time | ✅ | — | Provenance time anchor | — |
| `metadata.createdBy` | object | ✅ | — | Libra ActorRef mapping | `risk.level` |
| `metadata.target.repo` | object | ✅ | — | git clone target | `execution.*` |
| `metadata.target.baseRef` | string | ✅ | — | Run.commit baseline | `provenance.*` |
| `intent.summary` | string | ✅ | — | Task.title + PR title | — |
| `intent.problemStatement` | string | ✅ | — | Intent.prompt (immutable) | — |
| `intent.changeType` | enum | ✅ | `unknown` | Task.goal, influences qualityGates | `acceptance.qualityGates` |
| `intent.objectives[]` | array | ✅ | — | Generates child Tasks and PlanSteps | `libra.planGeneration.decompositionMode` |
| `intent.inScope[]` | array | ✅ | — | Task.constraints, ACL check | `security.toolAcl` |
| `intent.outOfScope[]` | array | — | `[]` | Task.constraints, scope-creep detection | `execution.replan.triggers` |
| `intent.touchHints` | object | — | `{}` | ContextSnapshot.items generation | `evidence.strategy` |
| `acceptance.successCriteria[]` | array | ✅ | — | Task.acceptance_criteria | — |
| `acceptance.verificationPlan` | object | ✅ | — | Four-stage gate Tasks | `artifacts.required` |
| `acceptance.qualityGates` | object | — | `{}` | Meta-policy constraints | `intent.changeType` |
| `constraints.security.networkPolicy` | enum | ✅ | `deny` | ToolInvocation pre-flight ACL | `security.toolAcl` |
| `constraints.security.dependencyPolicy` | enum | ✅ | `allow-with-review` | SCA gate trigger | `artifacts.required` |
| `constraints.privacy.dataClassesAllowed[]` | array | ✅ | `[public]` | ContextItem filtering | — |
| `constraints.licensing.allowedSpdx[]` | array | ✅ | `[]` | SCA licence check | `artifacts.required` |
| `constraints.resources.maxWallClockSeconds` | integer | — | `14400` | Run timeout + max_frames derivation | `libra.contextPipeline.maxFrames` |
| `risk.level` | enum | ✅ | `medium` | Forces gate configuration | `risk.humanInLoop` |
| `risk.humanInLoop.required` | boolean | ✅ | `false` | releaseChecks require-approvers | `risk.level` |
| `evidence.strategy` | enum | ✅ | `repo-first` | ContextSnapshot.selection_strategy | `evidence.domainAllowlistMode` |
| `evidence.domainAllowlistMode` | enum | ✅ | `allowlist-only` | URL ContextItem domain check | `evidence.allowedDomains` |
| `security.toolAcl.allow[]` | array | ✅ | — | ToolInvocation pre-flight intercept | `constraints.security.networkPolicy` |
| `security.secrets.policy` | enum | ✅ | `deny-all` | Run environment secret injection control | — |
| `security.promptInjection.*` | object | ✅ | — | ContextFrame trust tagging | `evidence.strategy` |
| `security.outputHandling.noDirectEval` | boolean | ✅ | `true` | PatchSet AST scan | — |
| `execution.retry.maxRetries` | integer | ✅ | `3` | Decision.Retry count limit | `execution.replan.triggers` |
| `execution.replan.triggers[]` | array | — | `[...]` | Plan.new_revision() trigger | `libra.decisionPolicy` |
| `execution.concurrency.maxParallelTasks` | integer | — | `4` | Parallel Task count limit | `constraints.resources` |
| `artifacts.required[]` | array | ✅ | — | Evidence ArtifactRef check | `acceptance.verificationPlan` |
| `artifacts.retention.days` | integer | — | `90` | ArtifactRef.expires_at | `constraints.privacy.retentionDays` |
| `provenance.requireSlsaProvenance` | boolean | ✅ | `true` | Provenance object creation | `provenance.bindings` |
| `provenance.requireSbom` | boolean | ✅ | `true` | securityChecks sbom check | `artifacts.required` |
| `provenance.transparencyLog.mode` | enum | ✅ | `rekor` | Rekor submission + transparency-proof | `libra.decisionPolicy.rollbackOnProvenanceFail` |
| `provenance.bindings.embedIntentSpecDigest` | boolean | ✅ | `true` | Provenance.externalParameters | — |
| `lifecycle.status` | enum | ✅ | `active` | Intent.status mapping | — |
| `lifecycle.changeLog[]` | array | ✅ | `[]` | Replan audit log | `execution.replan.triggers` |
| `libra.objectStore` | object | — | defaults | git object storage config | — |
| `libra.contextPipeline` | object | — | defaults | ContextPipeline frame management | `constraints.resources.maxWallClockSeconds` |
| `libra.planGeneration` | object | — | defaults | Task DAG generation strategy | `intent.objectives` |
| `libra.runPolicy` | object | — | defaults | Run execution detail | — |
| `libra.actorMapping` | object | — | defaults | Libra ActorRef mapping | `metadata.createdBy` |
| `libra.decisionPolicy` | object | — | defaults | Decision type policy | `risk.level` |

---

## 5. Example 1 — Minimal (low-risk bugfix)

**Scenario:** A null pointer dereference in a TypeScript service causes HTTP 500 errors. Only a single file's input-validation logic is affected; no new dependencies, no API structure changes.

**Key parameter choices:**
- `risk.level = low` — no human approval required
- `constraints.security.dependencyPolicy = no-new` — no dependency introduction
- `provenance.requireSbom = false` — SBOM not required at this risk level
- `provenance.transparencyLog.mode = none` — no transparency log
- `execution.concurrency.maxParallelTasks = 2` — two objectives, can run in parallel
- `libra.contextPipeline.maxFrames = 32` — small context window for a small task

See files:
- [`intentspec_minimal.json`](intentspec_minimal.json) — machine-readable, for testing and validation
- [`intentspec_minimal.yaml`](intentspec_minimal.yaml) — human-readable YAML representation

---

## 6. Example 2 — Typical (medium-risk new feature)

**Scenario:** A Python API service adds an optional `fields` query parameter to `GET /v2/report` for field-allowlist filtering. Multiple files are involved; contract tests and security scanning are required.

**Key parameter choices:**
- `risk.level = medium` — 1 approver required
- `constraints.security.dependencyPolicy = allow-with-review` — new deps allowed but need SCA
- `provenance.requireSbom = true` — SBOM required
- `provenance.transparencyLog.mode = rekor` — connected to Rekor
- `evidence.strategy = pinned-official` — white-listed official documentation allowed
- `libra.contextPipeline.maxFrames = 64` — medium complexity

See files:
- [`intentspec_typical.json`](intentspec_typical.json)
- [`intentspec_typical.yaml`](intentspec_typical.yaml)

---

## 7. Example 3 — High-assurance (high-risk security fix)

**Scenario:** An auth middleware has a conditional JWT bypass vulnerability (CVE). The fix requires full auditability: SBOM + SLSA provenance + Rekor inclusion proof, and four-eyes approval by the security team.

**Key parameter choices:**
- `risk.level = high` — forces `humanInLoop.minApprovers = 2`
- `constraints.security.dependencyPolicy = no-new` — no new dependencies for security fixes
- `execution.concurrency.maxParallelTasks = 1` — serial execution for easier auditing
- `libra.contextPipeline.maxFrames = 64` — controls information exposure surface
- `libra.decisionPolicy.rollbackOnProvenanceFail = true` — rollback on provenance failure
- `evidence.blockedDomains = ["*"]` — combined with `allowedDomains` for a strict whitelist

See files:
- [`intentspec_high_assurance.json`](intentspec_high_assurance.json)
- [`intentspec_high_assurance.yaml`](intentspec_high_assurance.yaml)

---

## 8. Example Parameter Comparison

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
| Number of securityChecks | `0` | `2` | `4` |
| Number of releaseChecks | `0` | `3` | `4` |
| Number of required artifacts | `2` | `7` | `8` |
