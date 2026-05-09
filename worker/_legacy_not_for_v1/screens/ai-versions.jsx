/* global React, ScreenWithChrome, StatusPill, fmtDateTime */
// AI Versions — Intent dossier
// Surfaces the full agent object model: Thread → Intent (+ IntentSpec) →
// Plans (execution + test) → Tasks → Runs → PatchSets, plus Events
// (IntentEvent, ToolInvocation, Evidence, Decision, ContextFrame, RunUsage)
// and the Libra Scheduler projection.

const REPO2 = () => window.LIBRA_API.repository;
const VERS  = () => window.LIBRA_API.ai_versions;
const API   = () => window.LIBRA_API;

// ─────────────────────────────────────────────────────────────────────────
// LIST  — thread of intents, with summary metrics per intent
// ─────────────────────────────────────────────────────────────────────────
function AIVersionsList() {
  const versions = VERS();
  const T = API().thread;
  const intent = API().intent_head;
  const plans = API().plans;
  const tasks = API().tasks;
  const runs  = API().runs;
  const patchsets = API().patchsets;
  const usage = API().run_usage;

  const totalTokens = usage.reduce((s, u) => s + u.tokens_in + u.tokens_out, 0);
  const totalCost   = usage.reduce((s, u) => s + u.cost_usd, 0);

  return (
    <ScreenWithChrome
      active="versions"
      repo={REPO2()}
      crumbs={["kepler-ledger", "AI versions", T.title]}
      actions={
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>
          {T.intents.length} revisions · {plans.length} plans · {runs.length} runs · {versions.length} patchsets
        </span>
      }
    >
      {/* Thread header */}
      <div style={{ padding: "22px 28px 18px", borderBottom: "1px solid var(--paper-line)" }}>
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-end", gap: 24 }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 6 }}>
              <span className="lb-eyebrow">Intent thread</span>
              <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>{T.thread_id}</span>
              <Tag>{intent.spec.intent.change_type}</Tag>
              <Tag tone={intent.spec.risk.level}>risk · {intent.spec.risk.level}</Tag>
              <StatusPill kind="proposed" />
            </div>
            <h1 className="lb-h1" style={{ fontSize: 22, fontWeight: 500 }}>{T.title}</h1>
            <div className="lb-meta" style={{ marginTop: 6 }}>
              {intent.spec.intent.in_scope.join(" · ")} <span style={{ color: "var(--ink-faint)" }}>·</span>{" "}
              <span className="lb-mono">{T.current_intent_id}</span> head
            </div>
          </div>
          <div style={{ display: "flex", gap: 18 }}>
            <Stat label="Tokens" value={(totalTokens / 1000).toFixed(1) + "K"} detail="across runs" />
            <Stat label="Cost" value={"$" + totalCost.toFixed(4)} detail="provider est." />
            <Stat label="Wall-clock" value={Math.round(usage.reduce((s, u) => s + u.wall_clock_seconds, 0) / 60) + "m"} detail="aggregate" />
          </div>
        </div>

        {/* Participants strip */}
        <div style={{ display: "flex", gap: 16, marginTop: 14, fontFamily: "var(--mono)", fontSize: 11, color: "var(--ink-mid)" }}>
          {T.participants.map((p, i) => (
            <span key={i}>
              <span style={{ color: "var(--ink-soft)" }}>{p.role}</span>{" "}
              <span style={{ color: "var(--ink)" }}>{p.id}</span>
            </span>
          ))}
        </div>
      </div>

      {/* Intent revision strip */}
      <div style={{
        display: "flex", alignItems: "center", padding: "10px 28px",
        borderBottom: "1px solid var(--paper-line)", gap: 6, background: "var(--paper-deep)",
      }}>
        <span className="lb-eyebrow" style={{ marginRight: 6 }}>Intent revisions</span>
        {T.intents.map((r) => (
          <span key={r.intent_id} style={{
            display: "inline-flex", alignItems: "center", gap: 6,
            padding: "4px 10px", borderRadius: "var(--r-1)",
            border: `1px solid ${r.is_head ? "var(--ink)" : "var(--paper-line)"}`,
            background: r.is_head ? "var(--ink)" : "var(--paper)",
            color: r.is_head ? "var(--paper)" : "var(--ink-mid)",
            fontFamily: "var(--mono)", fontSize: 10.5,
          }}>
            r{r.ordinal} · {r.intent_id}
            <span style={{ fontSize: 9.5, color: r.is_head ? "var(--gold-soft)" : "var(--ink-soft)" }}>
              {r.link_reason}
            </span>
          </span>
        ))}
        <div style={{ flex: 1 }} />
        <span className="lb-eyebrow">Stage</span>
        <Tag>{API().scheduler.active_dag_stage}_dag</Tag>
      </div>

      {/* Plan + Task summary band */}
      <div style={{ padding: "16px 28px", borderBottom: "1px solid var(--paper-line)", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 24 }}>
        {plans.map((p) => (
          <PlanCard key={p.plan_id} plan={p} tasks={tasks.filter((t) => p.steps.some((s) => s.step_id === t.origin_step_id))} runs={runs} />
        ))}
      </div>

      {/* Patchset / version table */}
      <div style={{
        display: "grid", gridTemplateColumns: "150px 1fr 130px 120px 140px 110px",
        padding: "10px 28px", borderBottom: "1px solid var(--paper-line)",
        fontFamily: "var(--sans)", fontSize: 10.5, letterSpacing: "0.12em",
        textTransform: "uppercase", color: "var(--ink-soft)", background: "var(--paper-deep)",
      }}>
        <span>PatchSet · run</span>
        <span>Prompt · origin</span>
        <span>Diff</span>
        <span>Tests</span>
        <span>Created · model</span>
        <span>Status</span>
      </div>
      <div style={{ flex: 1, overflow: "auto" }}>
        {versions.map((v, i) => (
          <VersionRow key={v.version_id} v={v} idx={i}
            run={runs.find((r) => patchsets.find((ps) => ps.patchset_id.endsWith(v.version_id.replace("av_", ""))) ? null : null) || runs[0]}
          />
        ))}
      </div>
    </ScreenWithChrome>
  );
}

function PlanCard({ plan, tasks, runs }) {
  const stepRunCounts = plan.steps.map((s) => {
    const t = tasks.find((tk) => tk.origin_step_id === s.step_id);
    const rs = t ? runs.filter((r) => r.task_id === t.task_id) : [];
    return { step: s, task: t, runs: rs };
  });
  return (
    <div style={{
      border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)",
      background: "var(--paper)", padding: "14px 16px",
    }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: 8, marginBottom: 8 }}>
        <span className="lb-eyebrow">{plan.kind} plan</span>
        <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>{plan.plan_id}</span>
        {plan.parents.length > 0 && (
          <span className="lb-mono" style={{ fontSize: 10, color: "var(--ink-faint)" }}>
            ← {plan.parents.join(", ")}
          </span>
        )}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
        {stepRunCounts.map(({ step, task, runs }) => (
          <div key={step.step_id} style={{
            display: "grid", gridTemplateColumns: "16px 1fr auto auto", gap: 8,
            alignItems: "center", padding: "4px 0",
            borderBottom: "1px dashed var(--paper-edge)",
          }}>
            <StepGlyph status={step.status} />
            <div style={{ minWidth: 0 }}>
              <div style={{ fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink)" }}>
                {step.title}
              </div>
              <div className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)" }}>
                {step.step_id}
                {step.depends_on.length > 0 && <> · deps {step.depends_on.join(", ")}</>}
              </div>
            </div>
            <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-mid)" }}>
              {task ? task.task_id : "—"}
            </span>
            <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>
              {runs.length} run{runs.length === 1 ? "" : "s"}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function StepGlyph({ status }) {
  const map = {
    completed: { ch: "●", c: "var(--good)" },
    running:   { ch: "◐", c: "var(--gold)" },
    pending:   { ch: "○", c: "var(--ink-faint)" },
    failed:    { ch: "✕", c: "var(--bad)" },
  };
  const m = map[status] || map.pending;
  return <span style={{ color: m.c, fontSize: 12, lineHeight: 1, textAlign: "center" }}>{m.ch}</span>;
}

function Stat({ label, value, detail }) {
  return (
    <div style={{ minWidth: 110 }}>
      <div className="lb-eyebrow" style={{ marginBottom: 4 }}>{label}</div>
      <div style={{ fontFamily: "var(--serif)", fontSize: 24, fontWeight: 500, color: "var(--ink)", lineHeight: 1 }}>{value}</div>
      <div className="lb-meta" style={{ marginTop: 4 }}>{detail}</div>
    </div>
  );
}

function Tag({ children, tone }) {
  const toneMap = {
    high:   { fg: "var(--bad)",  border: "var(--bad)" },
    medium: { fg: "var(--gold)", border: "var(--gold)" },
    low:    { fg: "var(--good)", border: "var(--good)" },
  };
  const t = toneMap[tone] || { fg: "var(--ink-mid)", border: "var(--paper-line)" };
  return (
    <span style={{
      fontFamily: "var(--sans)", fontSize: 10, letterSpacing: "0.08em",
      textTransform: "uppercase", padding: "2px 8px", borderRadius: "var(--r-1)",
      border: `1px solid ${t.border}`, color: t.fg,
    }}>{children}</span>
  );
}

function VersionRow({ v, idx }) {
  return (
    <div style={{
      display: "grid", gridTemplateColumns: "150px 1fr 130px 120px 140px 110px",
      padding: "14px 28px", borderBottom: "1px solid var(--paper-edge)",
      alignItems: "center", background: idx === 0 ? "var(--paper-deep)" : "transparent",
    }}>
      <div>
        <div className="lb-mono" style={{ fontSize: 12, color: "var(--ink)", fontWeight: 600 }}>{v.version_id}</div>
        {v.parent_version_id && (
          <div className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)", marginTop: 2 }}>
            ← {v.parent_version_id}
          </div>
        )}
      </div>
      <div style={{ minWidth: 0, paddingRight: 14 }}>
        <div style={{
          fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink)", lineHeight: 1.4,
          overflow: "hidden", textOverflow: "ellipsis", display: "-webkit-box",
          WebkitLineClamp: 2, WebkitBoxOrient: "vertical",
        }}>
          “{v.prompt_excerpt}”
        </div>
        <div style={{ display: "flex", gap: 10, marginTop: 6, fontFamily: "var(--sans)", fontSize: 10.5, color: "var(--ink-soft)" }}>
          <span>{v.origin.replace("_", " ")}</span>
          <span>·</span>
          <span>conf {v.confidence.toFixed(2)}</span>
        </div>
      </div>
      <div className="lb-mono" style={{ fontSize: 11.5 }}>
        <span style={{ color: "var(--good)" }}>+{v.diff_stats.lines_added}</span>
        <span style={{ color: "var(--ink-soft)" }}> · </span>
        <span style={{ color: "var(--warn)" }}>−{v.diff_stats.lines_removed}</span>
        <div style={{ color: "var(--ink-soft)", fontSize: 10, marginTop: 2 }}>{v.diff_stats.hunks} hunks</div>
      </div>
      <div className="lb-mono" style={{ fontSize: 11.5, color: v.tests_run ? "var(--ink)" : "var(--ink-soft)" }}>
        {v.tests_run ? <>{v.tests_run.passed}/{v.tests_run.passed + v.tests_run.failed + v.tests_run.skipped}</> : "—"}
        <div style={{ fontSize: 10, color: "var(--ink-soft)", marginTop: 2 }}>
          {v.tests_run ? (v.tests_run.failed > 0 ? `${v.tests_run.failed} failed` : "all green") : "not run"}
        </div>
      </div>
      <div className="lb-meta" style={{ fontFamily: "var(--mono)", fontSize: 11 }}>
        {fmtDateTime(v.created_at)}
        <div style={{ fontSize: 10, color: "var(--ink-soft)", marginTop: 2 }}>{v.model}</div>
      </div>
      <div><StatusPill kind={v.status} /></div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// DETAIL — full Intent dossier for the head intent / proposed PatchSet
// ─────────────────────────────────────────────────────────────────────────
function AIVersionDetail() {
  const v        = VERS()[0];
  const intent   = API().intent_head;
  const spec     = intent.spec;
  const plans    = API().plans;
  const tasks    = API().tasks;
  const runs     = API().runs;
  const patchsets = API().patchsets;
  const provenance = API().provenance_head;
  const usage    = API().run_usage;
  const events   = API().intent_events;
  const tools    = API().tool_invocations;
  const evidence = API().evidence;
  const decisions = API().decisions;
  const frames   = API().context_frames;
  const sched    = API().scheduler;

  const headPatch = patchsets.find((p) => p.patchset_id === "ps_av_01H9K2") || patchsets[0];
  const headRun   = runs.find((r) => r.run_id === headPatch.run_id);

  return (
    <ScreenWithChrome
      active="versions"
      repo={REPO2()}
      crumbs={["kepler-ledger", "AI versions", intent.intent_id, v.version_id]}
      actions={
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>
          {intent.intent_id} · {v.version_id} · {v.status}
        </span>
      }
    >
      {/* INTENT HEADER */}
      <SectionHeader eyebrow="Intent · Snapshot" id={intent.intent_id} sub={`r${API().thread.intents.find(r => r.intent_id === intent.intent_id).ordinal} of ${API().thread.intents.length}`} />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)" }}>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 320px", gap: 24 }}>
          <div style={{ minWidth: 0 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
              <Tag>{spec.intent.change_type}</Tag>
              <Tag tone={spec.risk.level}>risk · {spec.risk.level}</Tag>
              <StatusPill kind="proposed" />
              <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>
                {spec.api_version} · v{spec.lifecycle.schema_version}
              </span>
            </div>
            <h1 className="lb-h1" style={{ fontSize: 22, fontWeight: 500 }}>{spec.intent.summary}</h1>
            <p style={{
              fontFamily: "var(--serif)", fontSize: 15, color: "var(--ink-mid)",
              margin: "10px 0 0", maxWidth: 720, lineHeight: 1.55, textWrap: "pretty",
              borderLeft: "2px solid var(--gold)", paddingLeft: 12, fontStyle: "italic",
            }}>
              “{intent.prompt}”
            </p>
          </div>
          <KVCard rows={[
            ["intent_id",      intent.intent_id],
            ["parents",        intent.parents.join(", ") || "—"],
            ["base_ref",       spec.metadata.target.base_ref],
            ["repo",           "git@…/ledger.git"],
            ["created_by",     intent.created_by.id],
            ["status",         spec.lifecycle.status],
            ["intentspec_id",  spec.metadata.id],
          ]} />
        </div>

        {/* Objectives + scope grid */}
        <div style={{ display: "grid", gridTemplateColumns: "1.4fr 1fr 1fr", gap: 18, marginTop: 22 }}>
          <FactCard label={`Objectives · ${spec.intent.objectives.length}`}>
            {spec.intent.objectives.map((o, i) => (
              <div key={i} style={{ display: "flex", gap: 8, alignItems: "baseline", padding: "4px 0", borderBottom: "1px dashed var(--paper-edge)" }}>
                <span className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)", width: 86 }}>{o.kind}</span>
                <span style={{ fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink)", flex: 1, lineHeight: 1.4 }}>{o.title}</span>
              </div>
            ))}
          </FactCard>
          <FactCard label={`In scope · ${spec.intent.in_scope.length}`}>
            {spec.intent.in_scope.map((s, i) => (
              <div key={i} className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink)", padding: "3px 0" }}>{s}</div>
            ))}
          </FactCard>
          <FactCard label={`Out of scope · ${spec.intent.out_of_scope.length}`}>
            {spec.intent.out_of_scope.map((s, i) => (
              <div key={i} className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink-soft)", padding: "3px 0", textDecoration: "line-through" }}>{s}</div>
            ))}
          </FactCard>
        </div>

        {/* Touch hints */}
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr 1fr", gap: 18, marginTop: 16 }}>
          <FactCard label="Touch · files">
            {spec.intent.touch_hints.files.map((f, i) => <Mono key={i}>{f}</Mono>)}
          </FactCard>
          <FactCard label="Touch · symbols">
            {spec.intent.touch_hints.symbols.map((f, i) => <Mono key={i}>{f}()</Mono>)}
          </FactCard>
          <FactCard label="Touch · APIs">
            {spec.intent.touch_hints.apis.map((f, i) => <Mono key={i}>{f}</Mono>)}
          </FactCard>
        </div>

        {/* Risk rationale */}
        <FactCard label={`Risk · ${spec.risk.level} · factors: ${spec.risk.factors.join(", ")}`} style={{ marginTop: 16 }}>
          <div style={{ fontFamily: "var(--serif)", fontSize: 13.5, color: "var(--ink-mid)", lineHeight: 1.5 }}>
            {spec.risk.rationale}
          </div>
          <div style={{ marginTop: 6, fontFamily: "var(--mono)", fontSize: 11, color: "var(--ink-soft)" }}>
            human_in_loop: required={String(spec.risk.human_in_loop.required)} · min_approvers={spec.risk.human_in_loop.min_approvers}
          </div>
        </FactCard>
      </div>

      {/* INTENTSPEC POLICY */}
      <SectionHeader eyebrow="IntentSpec · Policy" id={spec.metadata.id} sub="control plane" />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 18 }}>
        <FactCard label="Acceptance · success criteria">
          {spec.acceptance.success_criteria.map((s, i) => (
            <div key={i} style={{ display: "flex", gap: 8, padding: "3px 0" }}>
              <span style={{ color: "var(--gold)", fontFamily: "var(--mono)", fontSize: 12 }}>✓</span>
              <span style={{ fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink)", lineHeight: 1.45 }}>{s}</span>
            </div>
          ))}
          <div style={{ marginTop: 8, fontFamily: "var(--mono)", fontSize: 11, color: "var(--ink-soft)" }}>
            verification_plan: fast {spec.acceptance.verification_plan.fast_checks} · int {spec.acceptance.verification_plan.integration_checks} · sec {spec.acceptance.verification_plan.security_checks} · rel {spec.acceptance.verification_plan.release_checks}
          </div>
        </FactCard>
        <FactCard label="Constraints · resources & policy">
          <KVRow k="network_policy"      v={spec.constraints.security.network_policy} />
          <KVRow k="dependency_policy"   v={spec.constraints.security.dependency_policy} />
          <KVRow k="data_classes"        v={spec.constraints.privacy.data_classes_allowed.join(", ")} />
          <KVRow k="redaction_required"  v={String(spec.constraints.privacy.redaction_required)} />
          <KVRow k="allowed_spdx"        v={spec.constraints.licensing.allowed_spdx.join(", ")} />
          <KVRow k="language_runtime"    v={spec.constraints.platform.language_runtime} />
          <KVRow k="max_wall_clock"      v={spec.constraints.resources.max_wall_clock_seconds + "s"} />
          <KVRow k="max_cost_units"      v={spec.constraints.resources.max_cost_units.toLocaleString()} />
        </FactCard>

        <FactCard label="Evidence policy">
          <KVRow k="strategy"             v={spec.evidence.strategy} />
          <KVRow k="trust_tiers"          v={spec.evidence.trust_tiers.join(", ")} />
          <KVRow k="domain_allowlist"     v={spec.evidence.domain_allowlist_mode} />
          <KVRow k="allowed_domains"      v={spec.evidence.allowed_domains.join(", ")} />
          <KVRow k="blocked_domains"      v={spec.evidence.blocked_domains.join(", ") || "—"} />
          <KVRow k="min_citations"        v={spec.evidence.min_citations_per_decision} />
        </FactCard>
        <FactCard label="Security · tool ACL">
          {spec.security.tool_acl.allow.map((r, i) => (
            <div key={i} style={{ padding: "4px 0", borderBottom: "1px dashed var(--paper-edge)" }}>
              <span className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink)" }}>
                allow {r.tool}
              </span>
              <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>
                {" "}· {r.actions.join(", ")}
              </span>
            </div>
          ))}
          {spec.security.tool_acl.deny.map((r, i) => (
            <div key={i} style={{ padding: "4px 0" }}>
              <span className="lb-mono" style={{ fontSize: 11.5, color: "var(--bad)" }}>deny {r.tool}</span>
              <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>
                {" "}· {r.actions.join(", ")} · {Object.keys(r.constraints).join(", ")}
              </span>
            </div>
          ))}
          <div style={{ marginTop: 8, fontFamily: "var(--mono)", fontSize: 10.5, color: "var(--ink-soft)" }}>
            secrets {spec.security.secrets.policy} · output {spec.security.output_handling.encoding_policy} · no-eval {String(spec.security.output_handling.no_direct_eval)}
          </div>
        </FactCard>

        <FactCard label="Execution · retry & replan">
          <KVRow k="max_retries"        v={spec.execution.retry.max_retries} />
          <KVRow k="backoff_seconds"    v={spec.execution.retry.backoff_seconds} />
          <KVRow k="max_parallel_tasks" v={spec.execution.concurrency.max_parallel_tasks} />
          <KVRow k="replan_triggers"    v={spec.execution.replan.triggers.join(", ")} />
        </FactCard>
        <FactCard label="Provenance · supply chain">
          <KVRow k="slsa_provenance"   v={String(spec.provenance.require_slsa_provenance)} />
          <KVRow k="sbom"              v={String(spec.provenance.require_sbom)} />
          <KVRow k="transparency_log"  v={spec.provenance.transparency_log.mode} />
          <KVRow k="embed_intentspec"  v={String(spec.provenance.bindings.embed_intentspec_digest)} />
          <KVRow k="embed_evidence"    v={String(spec.provenance.bindings.embed_evidence_digests)} />
          <div style={{ marginTop: 8, fontFamily: "var(--mono)", fontSize: 10.5, color: "var(--ink-soft)" }}>
            artifacts required · {spec.artifacts.required.length} · retention {spec.artifacts.retention.days}d
          </div>
        </FactCard>
      </div>

      {/* ARTIFACTS REQUIRED */}
      <SectionHeader eyebrow="Artifacts · Required" id={`${spec.artifacts.required.length} entries`} sub="checked at gate stages" />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)" }}>
        <div style={{
          display: "grid",
          gridTemplateColumns: "1.2fr 1fr 1fr 80px",
          padding: "8px 12px", background: "var(--paper-deep)",
          fontFamily: "var(--sans)", fontSize: 10, letterSpacing: "0.12em",
          textTransform: "uppercase", color: "var(--ink-soft)",
          border: "1px solid var(--paper-line)", borderBottom: "none", borderRadius: "var(--r-2) var(--r-2) 0 0",
        }}>
          <span>name</span><span>stage</span><span>format</span><span>required</span>
        </div>
        <div style={{ border: "1px solid var(--paper-line)", borderRadius: "0 0 var(--r-2) var(--r-2)" }}>
          {spec.artifacts.required.map((a, i) => (
            <div key={i} style={{
              display: "grid", gridTemplateColumns: "1.2fr 1fr 1fr 80px",
              padding: "8px 12px",
              borderBottom: i === spec.artifacts.required.length - 1 ? "none" : "1px solid var(--paper-edge)",
              fontFamily: "var(--mono)", fontSize: 11.5, alignItems: "center",
            }}>
              <span style={{ color: "var(--ink)", fontWeight: 600 }}>{a.name}</span>
              <span style={{ color: "var(--ink-mid)" }}>{a.stage}</span>
              <span style={{ color: "var(--ink-soft)" }}>{a.format}</span>
              <span style={{ color: a.required ? "var(--gold)" : "var(--ink-soft)" }}>
                {a.required ? "required" : "optional"}
              </span>
            </div>
          ))}
        </div>
      </div>

      {/* PLAN PAIR */}
      <SectionHeader eyebrow="Plan set · execution + test" id={`${plans.length} heads`} sub={`stage barrier · ${sched.active_dag_stage}_dag`} />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 18 }}>
        {plans.map((p) => (
          <PlanCard
            key={p.plan_id}
            plan={p}
            tasks={tasks.filter((t) => p.steps.some((s) => s.step_id === t.origin_step_id))}
            runs={runs}
          />
        ))}
      </div>

      {/* TASK DAG */}
      <SectionHeader eyebrow="Tasks · execution DAG" id={`${tasks.length} tasks · active ${sched.active_task_id}`} sub="dependencies → topological" />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)" }}>
        <TaskTable tasks={tasks} runs={runs} active={sched.active_task_id} ready={sched.ready_queue} />
      </div>

      {/* RUNS + USAGE */}
      <SectionHeader eyebrow="Runs · attempts" id={`${runs.length} runs · active ${sched.active_run_id}`} sub="immutable execution envelopes" />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)" }}>
        <RunTable runs={runs} usage={usage} active={sched.active_run_id} />
      </div>

      {/* HEAD PATCHSET DIFF */}
      <SectionHeader
        eyebrow="PatchSet · head"
        id={headPatch.patchset_id}
        sub={`${v.version_id} · seq ${headPatch.sequence} · ${headPatch.format}`}
      />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)" }}>
        {/* metadata strip */}
        <div style={{
          display: "flex", alignItems: "center", gap: 18, padding: "10px 14px",
          background: "var(--paper-deep)", border: "1px solid var(--paper-line)",
          borderRadius: "var(--r-2) var(--r-2) 0 0", borderBottom: "none",
        }}>
          <span className="lb-mono" style={{ fontSize: 11.5 }}>
            <span style={{ color: "var(--good)" }}>+{v.diff_stats.lines_added}</span>
            <span style={{ color: "var(--ink-soft)" }}>  ·  </span>
            <span style={{ color: "var(--warn)" }}>−{v.diff_stats.lines_removed}</span>
            <span style={{ color: "var(--ink-soft)" }}>  ·  </span>
            <span style={{ color: "var(--ink-mid)" }}>{v.diff_stats.hunks} hunks</span>
          </span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>
            run {headRun.run_id} · commit {headRun.commit}
          </span>
          <div style={{ flex: 1 }} />
          <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>
            touched {headPatch.touched.join(", ")}
          </span>
        </div>
        <div style={{
          display: "grid", gridTemplateColumns: "1fr 1fr",
          border: "1px solid var(--paper-line)", borderRadius: "0 0 var(--r-2) var(--r-2)",
          minHeight: 280,
        }}>
          <DiffPane side="base"     sha={v.base_blob_sha} />
          <DiffPane side="proposed" sha="(pending)" />
        </div>
        <div style={{
          marginTop: 10, fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink-mid)",
          fontStyle: "italic", lineHeight: 1.5, paddingLeft: 12, borderLeft: "2px solid var(--paper-line)",
        }}>
          {headPatch.rationale}
        </div>
      </div>

      {/* TOOL INVOCATIONS */}
      <SectionHeader eyebrow="Tool invocations" id={`${tools.length} on record`} sub="ACL-checked · footprint logged" />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)" }}>
        <ToolTable tools={tools} />
      </div>

      {/* EVIDENCE + DECISIONS */}
      <SectionHeader eyebrow="Evidence & Decisions" id={`${evidence.length} evidence · ${decisions.length} decisions`} />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 18 }}>
        <FactCard label="Evidence">
          {evidence.map((e) => (
            <div key={e.evidence_id} style={{ padding: "6px 0", borderBottom: "1px dashed var(--paper-edge)" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink)", fontWeight: 600 }}>{e.kind}</span>
                <EvidenceStatus status={e.status} />
                <span className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)", marginLeft: "auto" }}>
                  {fmtDateTime(e.at)}
                </span>
              </div>
              <div style={{ fontFamily: "var(--serif)", fontSize: 12.5, color: "var(--ink-mid)", marginTop: 3 }}>{e.summary}</div>
              <div className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)", marginTop: 2 }}>
                {e.evidence_id} · run {e.run_id}{e.artifact ? ` · ${e.artifact.name}.${e.artifact.format.split("+")[0]}` : ""}
              </div>
            </div>
          ))}
        </FactCard>
        <FactCard label="Decisions">
          {decisions.map((d) => (
            <div key={d.decision_id} style={{ padding: "6px 0", borderBottom: "1px dashed var(--paper-edge)" }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink)", fontWeight: 600 }}>{d.kind}</span>
                <span className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)", marginLeft: "auto" }}>{fmtDateTime(d.at)}</span>
              </div>
              <div style={{ fontFamily: "var(--serif)", fontSize: 12.5, color: "var(--ink-mid)", marginTop: 3, lineHeight: 1.45 }}>{d.rationale}</div>
              <div className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)", marginTop: 2 }}>
                {d.decision_id} · {d.actor}{d.chosen_patchset_id ? ` · → ${d.chosen_patchset_id}` : ""}
              </div>
            </div>
          ))}
        </FactCard>
      </div>

      {/* CONTEXT FRAMES */}
      <SectionHeader eyebrow="Context frames" id={`${frames.length} frames · ${sched.live_context_window.length} live`} sub="immutable incremental context" />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)" }}>
        <FrameTable frames={frames} liveSet={new Set(sched.live_context_window)} />
      </div>

      {/* INTENT EVENTS */}
      <SectionHeader eyebrow="Intent events" id={`${events.length} events`} sub="lifecycle of the thread" />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)" }}>
        <EventStrip events={events} />
      </div>

      {/* PROVENANCE */}
      <SectionHeader eyebrow="Provenance · head run" id={provenance.provenance_id} sub={`SLSA ${provenance.slsa_level}`} />
      <div style={{ padding: "0 28px 22px", borderBottom: "1px solid var(--paper-line)", display: "grid", gridTemplateColumns: "1fr 1fr", gap: 18 }}>
        <FactCard label="Provider · model">
          <KVRow k="provider"   v={provenance.provider} />
          <KVRow k="model"      v={provenance.model} />
          <KVRow k="builder_id" v={provenance.builder_id} />
          <KVRow k="run_id"     v={provenance.run_id} />
        </FactCard>
        <FactCard label="Parameters · binding">
          <KVRow k="temperature"     v={provenance.parameters.temperature} />
          <KVRow k="top_p"           v={provenance.parameters.top_p} />
          <KVRow k="max_output"      v={provenance.parameters.max_output_tokens} />
          <KVRow k="seed"            v={provenance.parameters.seed} />
          <KVRow k="intentspec_digest" v={provenance.intentspec_digest} />
        </FactCard>
      </div>

      {/* SCHEDULER PROJECTION */}
      <SectionHeader eyebrow="Scheduler · Libra projection" id="rebuildable" />
      <div style={{ padding: "0 28px 28px" }}>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 18 }}>
          <FactCard label="Selection">
            <KVRow k="selected_plan_ids"   v={sched.selected_plan_ids.join(", ")} />
            <KVRow k="current_plan_heads"  v={sched.current_plan_heads.join(", ")} />
            <KVRow k="active_dag_stage"    v={sched.active_dag_stage} />
            <KVRow k="active_task_id"      v={sched.active_task_id} />
            <KVRow k="active_run_id"       v={sched.active_run_id} />
          </FactCard>
          <FactCard label="Queues · context">
            <KVRow k="ready_queue"          v={sched.ready_queue.join(", ")} />
            <KVRow k="parallel_groups"      v={sched.parallel_groups.map(g => "[" + g.join(",") + "]").join(" ")} />
            <KVRow k="live_context_window"  v={sched.live_context_window.join(", ")} />
          </FactCard>
        </div>
      </div>

      {/* CHANGELOG */}
      <SectionHeader eyebrow="Lifecycle · change log" id={`${spec.lifecycle.change_log.length} entries`} sub="append-only audit trail" />
      <div style={{ padding: "0 28px 28px" }}>
        <div style={{ display: "flex", flexDirection: "column", gap: 0, border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)" }}>
          {spec.lifecycle.change_log.map((c, i) => (
            <div key={i} style={{
              display: "grid", gridTemplateColumns: "180px 140px 1fr",
              padding: "10px 14px",
              borderBottom: i === spec.lifecycle.change_log.length - 1 ? "none" : "1px solid var(--paper-edge)",
              alignItems: "baseline",
            }}>
              <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>{fmtDateTime(c.at)}</span>
              <span className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink)" }}>{c.by}</span>
              <span style={{ fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink-mid)", lineHeight: 1.45 }}>
                <span style={{ color: "var(--gold)" }}>{c.reason}</span> — {c.diff_summary}
              </span>
            </div>
          ))}
        </div>
      </div>
    </ScreenWithChrome>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Sub-components
// ─────────────────────────────────────────────────────────────────────────
function SectionHeader({ eyebrow, id, sub }) {
  return (
    <div style={{
      padding: "20px 28px 12px",
      display: "flex", alignItems: "baseline", gap: 12,
    }}>
      <span className="lb-eyebrow">{eyebrow}</span>
      <span style={{ flex: 1, height: 1, background: "var(--paper-line)" }} />
      {id && <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink)" }}>{id}</span>}
      {sub && <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>· {sub}</span>}
    </div>
  );
}

function FactCard({ label, children, style }) {
  return (
    <div style={{
      border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)",
      padding: "12px 14px", background: "var(--paper)",
      ...(style || {}),
    }}>
      <div className="lb-eyebrow" style={{ marginBottom: 8 }}>{label}</div>
      {children}
    </div>
  );
}

function KVCard({ rows }) {
  return (
    <div style={{
      border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)",
      padding: "12px 14px", background: "var(--paper-deep)",
    }}>
      {rows.map(([k, v], i) => (
        <KVRow key={i} k={k} v={v} last={i === rows.length - 1} />
      ))}
    </div>
  );
}

function KVRow({ k, v, last }) {
  return (
    <div style={{
      display: "flex", justifyContent: "space-between", gap: 16, padding: "5px 0",
      borderBottom: last ? "none" : "1px solid var(--paper-edge)",
      fontFamily: "var(--mono)", fontSize: 11,
    }}>
      <span style={{ color: "var(--ink-soft)" }}>{k}</span>
      <span style={{ color: "var(--ink)", textAlign: "right", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: 240 }}>{v}</span>
    </div>
  );
}

function Mono({ children }) {
  return <div className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink)", padding: "3px 0" }}>{children}</div>;
}

function TaskTable({ tasks, runs, active, ready }) {
  const readySet = new Set(ready);
  return (
    <div style={{ border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)" }}>
      <div style={{
        display: "grid", gridTemplateColumns: "100px 1fr 130px 140px 90px 90px",
        padding: "8px 12px", background: "var(--paper-deep)",
        fontFamily: "var(--sans)", fontSize: 10, letterSpacing: "0.12em",
        textTransform: "uppercase", color: "var(--ink-soft)",
        borderBottom: "1px solid var(--paper-line)",
      }}>
        <span>task_id</span><span>title</span><span>origin_step</span><span>depends_on</span><span>runs</span><span>status</span>
      </div>
      {tasks.map((t, i) => (
        <div key={t.task_id} style={{
          display: "grid", gridTemplateColumns: "100px 1fr 130px 140px 90px 90px",
          padding: "9px 12px", alignItems: "center",
          borderBottom: i === tasks.length - 1 ? "none" : "1px solid var(--paper-edge)",
          background: t.task_id === active ? "var(--paper-deep)" : "transparent",
          borderLeft: t.task_id === active ? "2px solid var(--gold)" : "2px solid transparent",
        }}>
          <span className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink)", fontWeight: 600 }}>{t.task_id}</span>
          <span style={{ fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink)" }}>{t.title}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{t.origin_step_id}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>{t.dependencies.join(", ") || "—"}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{t.run_count}</span>
          <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <StepGlyph status={t.status} />
            <span className="lb-mono" style={{ fontSize: 10.5, color: readySet.has(t.task_id) ? "var(--gold)" : "var(--ink-mid)" }}>
              {readySet.has(t.task_id) ? "ready" : t.status}
            </span>
          </span>
        </div>
      ))}
    </div>
  );
}

function RunTable({ runs, usage, active }) {
  const usageById = new Map(usage.map((u) => [u.run_id, u]));
  return (
    <div style={{ border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)" }}>
      <div style={{
        display: "grid", gridTemplateColumns: "110px 90px 90px 110px 90px 90px 130px 90px",
        padding: "8px 12px", background: "var(--paper-deep)",
        fontFamily: "var(--sans)", fontSize: 10, letterSpacing: "0.12em",
        textTransform: "uppercase", color: "var(--ink-soft)",
        borderBottom: "1px solid var(--paper-line)",
      }}>
        <span>run_id</span><span>task</span><span>retry</span><span>started</span><span>dur</span><span>tokens</span><span>cost · patches</span><span>status</span>
      </div>
      {runs.map((r, i) => {
        const u = usageById.get(r.run_id);
        return (
          <div key={r.run_id} style={{
            display: "grid", gridTemplateColumns: "110px 90px 90px 110px 90px 90px 130px 90px",
            padding: "9px 12px", alignItems: "center",
            borderBottom: i === runs.length - 1 ? "none" : "1px solid var(--paper-edge)",
            background: r.run_id === active ? "var(--paper-deep)" : "transparent",
            borderLeft: r.run_id === active ? "2px solid var(--gold)" : "2px solid transparent",
          }}>
            <span className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink)", fontWeight: 600 }}>{r.run_id}</span>
            <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{r.task_id}</span>
            <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>#{r.retry_index}</span>
            <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>{r.started_at.slice(11, 19)}</span>
            <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{r.duration_seconds}s</span>
            <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{u ? ((u.tokens_in + u.tokens_out) / 1000).toFixed(1) + "K" : "—"}</span>
            <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>
              {u ? `$${u.cost_usd.toFixed(4)}` : "—"} · {r.patchset_count}
            </span>
            <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <StepGlyph status={r.status === "succeeded" ? "completed" : r.status === "running" ? "running" : "failed"} />
              <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-mid)" }}>{r.status}</span>
            </span>
          </div>
        );
      })}
    </div>
  );
}

function ToolTable({ tools }) {
  return (
    <div style={{ border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)" }}>
      <div style={{
        display: "grid", gridTemplateColumns: "100px 90px 130px 90px 1fr 80px",
        padding: "8px 12px", background: "var(--paper-deep)",
        fontFamily: "var(--sans)", fontSize: 10, letterSpacing: "0.12em",
        textTransform: "uppercase", color: "var(--ink-soft)",
        borderBottom: "1px solid var(--paper-line)",
      }}>
        <span>id</span><span>run</span><span>tool</span><span>action</span><span>args / footprint</span><span>exit</span>
      </div>
      {tools.map((t, i) => (
        <div key={t.invocation_id} style={{
          display: "grid", gridTemplateColumns: "100px 90px 130px 90px 1fr 80px",
          padding: "8px 12px", alignItems: "center",
          borderBottom: i === tools.length - 1 ? "none" : "1px solid var(--paper-edge)",
        }}>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink)", fontWeight: 600 }}>{t.invocation_id}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{t.run_id}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink)" }}>{t.tool}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{t.action}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {t.args_summary}
          </span>
          <span className="lb-mono" style={{ fontSize: 11, color: t.exit_code === 0 ? "var(--good)" : "var(--bad)" }}>
            {t.exit_code === 0 ? "ok" : "x" + t.exit_code}
          </span>
        </div>
      ))}
    </div>
  );
}

function FrameTable({ frames, liveSet }) {
  return (
    <div style={{ border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)" }}>
      <div style={{
        display: "grid", gridTemplateColumns: "90px 130px 90px 70px 1fr",
        padding: "8px 12px", background: "var(--paper-deep)",
        fontFamily: "var(--sans)", fontSize: 10, letterSpacing: "0.12em",
        textTransform: "uppercase", color: "var(--ink-soft)",
        borderBottom: "1px solid var(--paper-line)",
      }}>
        <span>id</span><span>kind</span><span>trust</span><span>state</span><span>summary</span>
      </div>
      {frames.map((f, i) => (
        <div key={f.frame_id} style={{
          display: "grid", gridTemplateColumns: "90px 130px 90px 70px 1fr",
          padding: "9px 12px", alignItems: "center",
          borderBottom: i === frames.length - 1 ? "none" : "1px solid var(--paper-edge)",
          background: liveSet.has(f.frame_id) ? "var(--paper-deep)" : "transparent",
        }}>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink)", fontWeight: 600 }}>{f.frame_id}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{f.kind}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{f.trust}</span>
          <span className="lb-mono" style={{ fontSize: 10.5, color: f.protected ? "var(--gold)" : liveSet.has(f.frame_id) ? "var(--good)" : "var(--ink-soft)" }}>
            {f.protected ? "protected" : liveSet.has(f.frame_id) ? "live" : "evicted"}
          </span>
          <span style={{ fontFamily: "var(--serif)", fontSize: 12.5, color: "var(--ink-mid)" }}>{f.summary}</span>
        </div>
      ))}
    </div>
  );
}

function EventStrip({ events }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 0, border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)" }}>
      {events.map((e, i) => (
        <div key={e.event_id} style={{
          display: "grid", gridTemplateColumns: "90px 180px 130px 130px 1fr",
          padding: "9px 14px", alignItems: "baseline",
          borderBottom: i === events.length - 1 ? "none" : "1px solid var(--paper-edge)",
        }}>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>{e.event_id}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>{fmtDateTime(e.at)}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{e.intent_id}</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink)" }}>{e.actor}</span>
          <span style={{ fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink)" }}>
            <span style={{ color: "var(--gold)" }}>{e.kind}</span>
            {e.next_intent_id && <span className="lb-mono" style={{ color: "var(--ink-soft)", fontSize: 11 }}>{" "}→ {e.next_intent_id}</span>}
          </span>
        </div>
      ))}
    </div>
  );
}

function EvidenceStatus({ status }) {
  const map = {
    passed:  { ch: "✓", c: "var(--good)" },
    failed:  { ch: "✕", c: "var(--bad)" },
    running: { ch: "◐", c: "var(--gold)" },
  };
  const m = map[status] || { ch: "·", c: "var(--ink-soft)" };
  return (
    <span className="lb-mono" style={{ fontSize: 10.5, color: m.c }}>
      {m.ch} {status}
    </span>
  );
}

function DiffPane({ side, sha }) {
  const isBase = side === "base";
  const lines = isBase ? [
    { n: 17, k: "ctx", txt: "export async function postJournalEntry(" },
    { n: 18, k: "ctx", txt: "  entry: JournalEntry," },
    { n: 19, k: "del", txt: "  opts: { retries?: number } = {}," },
    { n: 20, k: "ctx", txt: "): Promise<{ ok: true; sha: string }> {" },
    { n: 21, k: "ctx", txt: "  const sha = await signer.sign(entry, SIGNING_KEY);" },
    { n: 22, k: "ctx", txt: "  const row = {" },
    { n: 23, k: "ctx", txt: "    ...entry," },
    { n: 24, k: "ctx", txt: "    sha," },
    { n: 25, k: "del", txt: '    posted_by: "m.ostrowski@…",' },
    { n: 26, k: "ctx", txt: "  };" },
    { n: 27, k: "del", txt: '  return db.insert("journal", row);' },
    { n: 28, k: "ctx", txt: "}" },
  ] : [
    { n: 17, k: "ctx", txt: "export async function postJournalEntry(" },
    { n: 18, k: "ctx", txt: "  entry: JournalEntry," },
    { n: 19, k: "add", txt: "  opts: { retries?: number; idempotent?: boolean } = {}," },
    { n: 20, k: "ctx", txt: "): Promise<{ ok: true; sha: string }> {" },
    { n: 21, k: "ctx", txt: "  const sha = await signer.sign(entry, SIGNING_KEY);" },
    { n: 22, k: "ctx", txt: "  const row = {" },
    { n: 23, k: "ctx", txt: "    ...entry," },
    { n: 24, k: "ctx", txt: "    sha," },
    { n: 25, k: "add", txt: "    posted_by: currentActor()," },
    { n: 26, k: "add", txt: "    idempotency_key: entry.idempotency_key ?? null," },
    { n: 27, k: "ctx", txt: "  };" },
    { n: 28, k: "add", txt: "  return withRetry(opts.retries, () =>" },
    { n: 29, k: "add", txt: '    db.insert("journal", row));' },
    { n: 30, k: "ctx", txt: "}" },
  ];
  return (
    <div style={{
      minWidth: 0, borderRight: isBase ? "1px solid var(--paper-line)" : "none",
      display: "flex", flexDirection: "column", background: "var(--paper)",
    }}>
      <div style={{
        padding: "8px 16px", borderBottom: "1px solid var(--paper-line)",
        background: "var(--paper-deep)", display: "flex",
        alignItems: "center", justifyContent: "space-between",
      }}>
        <span className="lb-eyebrow">{isBase ? "Base · main" : "Proposed · av_01H9K2"}</span>
        <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>{sha}</span>
      </div>
      <div className="lb-code" style={{ flex: 1, overflow: "auto", padding: "8px 0" }}>
        {lines.map((l, i) => <DiffLine key={i} {...l} />)}
      </div>
    </div>
  );
}

function DiffLine({ n, k, txt }) {
  const tones = {
    ctx: { bg: "transparent", marker: " ", color: "var(--ink-deep)", marker_color: "var(--ink-faint)" },
    add: { bg: "rgba(72, 108, 77, 0.10)", marker: "+", color: "var(--ink-deep)", marker_color: "var(--good)" },
    del: { bg: "rgba(140, 106, 43, 0.10)", marker: "−", color: "var(--ink-deep)", marker_color: "var(--warn)" },
  }[k];
  return (
    <div style={{ display: "flex", alignItems: "center", background: tones.bg, paddingLeft: 12, paddingRight: 16 }}>
      <span style={{ fontFamily: "var(--mono)", fontSize: 10.5, color: "var(--ink-faint)", width: 28, textAlign: "right", paddingRight: 8, userSelect: "none" }}>{n}</span>
      <span style={{ fontFamily: "var(--mono)", fontSize: 11, color: tones.marker_color, width: 12, userSelect: "none", fontWeight: k === "ctx" ? 400 : 700 }}>{tones.marker}</span>
      <span style={{ fontFamily: "var(--mono)", fontSize: 11.5, color: tones.color, whiteSpace: "pre", flex: 1, minWidth: 0, overflow: "hidden" }}>{txt}</span>
    </div>
  );
}

Object.assign(window, { AIVersionsList, AIVersionDetail });
