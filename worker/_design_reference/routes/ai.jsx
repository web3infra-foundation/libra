/* global React, AppShell, PageHeader, RefPicker, StatusPill,
   useHashRoute, useViewerMode, useBreakpoint, fmtRelative, fmtDateTime, shortSha */
// AI Object Browser — categories rail / object list / detail.
// Covers Snapshot, Event, and Libra-projection objects from the AOM reference.

const { useState, useMemo, useEffect } = React;

// ── Object catalogue ────────────────────────────────────────────────────────
const CATEGORIES = [
  {
    id: "snapshot", label: "Snapshot", desc: "Versioned, immutable",
    types: [
      { id: "thread",     label: "Thread"     },
      { id: "intent",     label: "Intent"     },
      { id: "intentspec", label: "IntentSpec" },
      { id: "plan",       label: "Plan"       },
      { id: "task",       label: "Task"       },
      { id: "run",        label: "Run"        },
      { id: "patchset",   label: "PatchSet"   },
      { id: "provenance", label: "Provenance" },
    ],
  },
  {
    id: "event", label: "Event", desc: "Append-only log",
    types: [
      { id: "intent_event",     label: "IntentEvent"    },
      { id: "tool_invocation",  label: "ToolInvocation" },
      { id: "evidence",         label: "Evidence"       },
      { id: "decision",         label: "Decision"       },
      { id: "context_frame",    label: "ContextFrame"   },
      { id: "run_usage",        label: "RunUsage"       },
    ],
  },
  {
    id: "projection", label: "Projection", desc: "Libra runtime view",
    types: [
      { id: "scheduler",  label: "Scheduler" },
    ],
  },
];

// ── Page entry ──────────────────────────────────────────────────────────────
function AiObjectsPage() {
  const route = useHashRoute();
  const refs = window.LIBRA_API.refs;
  const refName = route.params.ref || window.LIBRA_API.repository.default_branch;
  const setRef = (n) => route.updateParams({ ref: n === window.LIBRA_API.repository.default_branch ? null : n });

  const bp = useBreakpoint();

  // All object types are public.
  const visibleCats = CATEGORIES;

  const initialType = visibleCats[0]?.types[0]?.id || "intent";
  const type = route.params.type || initialType;
  const id   = route.params.id   || null;
  const setType = (t) => route.updateParams({ type: t, id: null });
  const setId   = (i) => route.updateParams({ id: i });

  // Mobile staged view
  const [stage, setStage] = useState("list"); // list | detail
  useEffect(() => { if (id) setStage("detail"); }, [id]);

  return (
    <AppShell>
      <PageHeader
        eyebrow="AI objects"
        title="Object browser"
        subtitle={<>Snapshot · Event · Libra projection</>}
        refPicker={<RefPicker refs={refs} value={refName} onChange={setRef} />}
      />
      <div style={{
        flex: 1, minHeight: 0, display: "flex",
        flexDirection: bp === "mobile" ? "column" : "row",
      }}>
        {/* Category rail — always visible on tablet+, shown above on mobile */}
        <CategoryRail visibleCats={visibleCats} type={type} onPick={(t) => { setType(t); setStage("list"); }} />

        {/* Object list */}
        {(bp !== "mobile" || stage === "list") && (
          <ObjectList type={type} pickedId={id} onPick={(i) => { setId(i); setStage("detail"); }} />
        )}

        {/* Detail */}
        {(bp !== "mobile" || stage === "detail") && (
          <ObjectDetail type={type} id={id} onBack={() => setStage("list")}/>
        )}
      </div>
    </AppShell>
  );
}

// ── Category rail ───────────────────────────────────────────────────────────
function CategoryRail({ visibleCats, type, onPick }) {
  const bp = useBreakpoint();
  if (bp === "mobile") {
    return (
      <nav aria-label="Object types" style={{
        borderBottom: "1px solid var(--paper-line)",
        background: "var(--paper-deep)",
        overflowX: "auto",
        whiteSpace: "nowrap",
      }}>
        <div style={{ display: "flex", padding: "8px 12px", gap: 6 }}>
          {visibleCats.flatMap(c => c.types).map(t => {
            const on = t.id === type;
            return (
              <button key={t.id} type="button" onClick={() => onPick(t.id)}
                aria-pressed={on}
                style={{
                  padding: "6px 12px",
                  borderRadius: 999,
                  fontFamily: "var(--sans)", fontSize: 12, fontWeight: 600,
                  background: on ? "var(--ink-deep)" : "var(--paper)",
                  color: on ? "var(--paper)" : "var(--ink-mid)",
                  border: "1px solid var(--paper-line)",
                }}>{t.label}</button>
            );
          })}
        </div>
      </nav>
    );
  }
  return (
    <nav aria-label="Object types" style={{
      width: 220, borderRight: "1px solid var(--paper-line)",
      background: "var(--paper-deep)", overflowY: "auto",
    }}>
      {visibleCats.map(c => (
        <div key={c.id} style={{ padding: "12px 0 6px" }}>
          <div className="lb-eyebrow" style={{ padding: "0 16px 6px", fontSize: 10 }}>
            {c.label}
            <span className="lb-meta" style={{ marginLeft: 8, fontSize: 10, color: "var(--ink-faint)", textTransform: "none", letterSpacing: 0 }}>
              · {c.desc}
            </span>
          </div>
          {c.types.map(t => {
            const on = t.id === type;
            return (
              <button key={t.id} type="button" onClick={() => onPick(t.id)}
                aria-current={on ? "page" : undefined}
                style={{
                  display: "flex", alignItems: "center", justifyContent: "space-between",
                  width: "100%",
                  padding: "7px 16px",
                  background: on ? "var(--paper)" : "transparent",
                  borderLeft: on ? "2px solid var(--gold)" : "2px solid transparent",
                  fontFamily: "var(--sans)", fontSize: 12.5,
                  fontWeight: on ? 600 : 500,
                  color: on ? "var(--ink-deep)" : "var(--ink-mid)",
                }}>
                <span>{t.label}</span>
              </button>
            );
          })}
        </div>
      ))}
    </nav>
  );
}

// ── Object list — adapter per type ─────────────────────────────────────────
function ObjectList({ type, pickedId, onPick }) {
  const bp = useBreakpoint();
  const items = listItemsFor(type);
  return (
    <section aria-label={`${type} list`} style={{
      width: bp === "mobile" ? "100%" : "clamp(260px, 28%, 340px)",
      borderRight: bp === "mobile" ? "0" : "1px solid var(--paper-line)",
      borderBottom: bp === "mobile" ? "1px solid var(--paper-line)" : "0",
      overflowY: "auto",
      background: "var(--paper)",
    }}>
      <header style={{ padding: "10px 16px", borderBottom: "1px solid var(--paper-line)",
                       background: "var(--paper-deep)" }}>
        <div className="lb-eyebrow" style={{ fontSize: 10 }}>List</div>
        <div className="lb-h2" style={{ fontSize: 13.5 }}>{prettyType(type)} <span className="lb-mono" style={{ color: "var(--ink-soft)", fontWeight: 500 }}>· {items.length}</span></div>
      </header>
      {items.length === 0 && <div className="lb-meta" style={{ padding: 24, textAlign: "center" }}>No items.</div>}
      {items.map(it => {
        const on = it.id === pickedId;
        return (
          <button key={it.id} type="button" onClick={() => onPick(it.id)}
            aria-current={on ? "true" : undefined}
            style={{
              display: "block", width: "100%", textAlign: "left",
              padding: "10px 16px",
              borderBottom: "1px solid var(--paper-edge)",
              background: on ? "var(--paper-deep)" : "transparent",
              borderLeft: on ? "2px solid var(--gold)" : "2px solid transparent",
            }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 8 }}>
              <span className="lb-mono" style={{ fontSize: 11.5, fontWeight: on ? 600 : 500, color: "var(--ink-deep)" }}>
                {it.id}
              </span>
              {it.pill}
            </div>
            <div style={{ marginTop: 4, fontSize: 12.5, color: "var(--ink)", lineHeight: 1.4,
                          display: "-webkit-box", WebkitLineClamp: 2, WebkitBoxOrient: "vertical", overflow: "hidden" }}>
              {it.title}
            </div>
            {it.meta && <div className="lb-meta" style={{ fontSize: 10.5, marginTop: 4 }}>{it.meta}</div>}
          </button>
        );
      })}
    </section>
  );
}

function listItemsFor(type) {
  const A = window.LIBRA_API;
  switch (type) {
    case "thread":     return [{ id: A.thread.thread_id, title: A.thread.title, meta: `${A.thread.intents.length} intents · ${A.thread.participants.length} participants`, pill: <StatusPill kind={A.thread.archived ? "superseded" : "accepted"}/> }];
    case "intent":     return [{ id: A.intent_head.intent_id, title: A.intent_head.prompt, meta: `${fmtRelative(A.intent_head.created_at)} · ${A.intent_head.created_by.id}`, pill: <StatusPill kind="proposed">{A.intent_head.status}</StatusPill> }];
    case "intentspec": return [{ id: A.intent_head.spec.metadata.id, title: A.intent_head.spec.intent.summary, meta: `${A.intent_head.spec.intent.change_type} · risk ${A.intent_head.spec.risk.level}`, pill: <StatusPill kind="info">{A.intent_head.spec.lifecycle.status}</StatusPill> }];
    case "plan":       return A.plans.map(p => ({ id: p.plan_id, title: `${p.kind === "execution" ? "Execution" : "Test"} plan · ${p.steps.length} steps`, meta: `${p.steps.filter(s => s.status === "completed").length}/${p.steps.length} done · ${fmtRelative(p.created_at)}`, pill: <StatusPill kind="info">{p.kind}</StatusPill> }));
    case "task":       return A.tasks.map(t => ({ id: t.task_id, title: t.title, meta: `step ${t.origin_step_id} · ${t.run_count} runs`, pill: <StatusPill kind={t.status}/> }));
    case "run":        return A.runs.map(r => ({ id: r.run_id, title: `Run for ${r.task_id}`, meta: `${fmtRelative(r.started_at)} · ${r.duration_seconds}s · ${r.patchset_count} patchset(s)`, pill: <StatusPill kind={r.status}/> }));
    case "patchset":   return A.patchsets.map(p => ({ id: p.patchset_id, title: p.touched.join(", "), meta: `+${p.lines_added} −${p.lines_removed} · ${p.hunks} hunks`, pill: <StatusPill kind="proposed">{p.format}</StatusPill> }));
    case "provenance": return [{ id: A.provenance_head.provenance_id, title: `${A.provenance_head.model} · ${A.provenance_head.provider}`, meta: `seed ${A.provenance_head.parameters.seed} · SLSA ${A.provenance_head.slsa_level}`, pill: <StatusPill kind="good">{A.provenance_head.slsa_level}</StatusPill> }];
    case "intent_event":    return A.intent_events.map(e => ({ id: e.event_id, title: `${e.kind} · ${e.intent_id}`, meta: `${fmtDateTime(e.at)} · ${e.actor}`, pill: <StatusPill kind="info">{e.kind}</StatusPill> }));
    case "tool_invocation": return A.tool_invocations.map(t => ({ id: t.invocation_id, title: `${t.tool} · ${t.action}`, meta: `${fmtDateTime(t.at)} · run ${t.run_id} · exit ${t.exit_code}`, pill: <StatusPill kind={t.exit_code === 0 ? "good" : "bad"}>{t.exit_code === 0 ? "ok" : "fail"}</StatusPill> }));
    case "evidence":        return A.evidence.map(e => ({ id: e.evidence_id, title: `${e.kind} · ${e.summary}`, meta: `${fmtDateTime(e.at)} · run ${e.run_id}`, pill: <StatusPill kind={e.status}/> }));
    case "decision":        return A.decisions.map(d => ({ id: d.decision_id, title: `${d.kind}`, meta: `${fmtDateTime(d.at)} · ${d.actor}`, pill: <StatusPill kind="info">{d.kind}</StatusPill> }));
    case "context_frame":   return A.context_frames.map(f => ({ id: f.frame_id, title: f.summary, meta: `${f.kind} · ${fmtDateTime(f.at)}${f.protected ? " · protected" : ""}`, pill: <StatusPill kind={f.protected ? "warn" : "info"}>{f.kind}</StatusPill> }));
    case "run_usage":       return A.run_usage.map(u => ({ id: u.usage_id, title: `${u.tokens_in.toLocaleString()} in · ${u.tokens_out.toLocaleString()} out`, meta: `run ${u.run_id} · $${u.cost_usd.toFixed(4)} · ${u.wall_clock_seconds}s`, pill: null }));
    case "scheduler":       return [{ id: "scheduler", title: `Active task ${A.scheduler.active_task_id} · stage ${A.scheduler.active_dag_stage}`, meta: `ready: ${A.scheduler.ready_queue.join(", ") || "—"}`, pill: <StatusPill kind="info">live</StatusPill> }];
    default: return [];
  }
}

const prettyType = (t) => CATEGORIES.flatMap(c => c.types).find(x => x.id === t)?.label || t;

// ── Detail panel ───────────────────────────────────────────────────────────
function ObjectDetail({ type, id, onBack }) {
  const bp = useBreakpoint();
  return (
    <section aria-label={`${type} detail`} style={{
      flex: 1, minWidth: 0, minHeight: 0, overflow: "auto",
      background: "var(--paper)",
    }}>
      {bp === "mobile" && (
        <button type="button" onClick={onBack}
          style={{
            display: "inline-flex", alignItems: "center", gap: 6,
            margin: "12px 16px 0",
            padding: "6px 10px",
            border: "1px solid var(--paper-line)", borderRadius: "var(--r-1)",
            background: "var(--paper-deep)", fontSize: 12, fontWeight: 600, color: "var(--ink-mid)",
          }}>← List</button>
      )}
      <div style={{ padding: "16px clamp(16px, 3vw, 28px) 64px" }}>
        {!id ? <DetailEmpty type={type}/> : renderDetail(type, id)}
      </div>
    </section>
  );
}

function DetailEmpty({ type }) {
  return (
    <div style={{ padding: 32 }}>
      <div className="lb-eyebrow">Detail</div>
      <p className="lb-meta">Pick a {prettyType(type)} from the list to inspect.</p>
    </div>
  );
}

// Tiny presentational primitives reused below
const KV = ({ k, v, mono }) => (
  <div style={{ display: "grid", gridTemplateColumns: "minmax(140px, 24%) 1fr", gap: 12, padding: "8px 0", borderBottom: "1px solid var(--paper-edge)" }}>
    <div className="lb-eyebrow" style={{ fontSize: 10 }}>{k}</div>
    <div className={mono ? "lb-mono" : ""} style={{ fontSize: 12.5, color: "var(--ink-deep)", wordBreak: "break-word" }}>{v ?? "—"}</div>
  </div>
);
const Card = ({ title, eyebrow, children, action }) => (
  <section style={{
    border: "1px solid var(--paper-line)", borderRadius: "var(--r-2)",
    background: "var(--paper)", marginBottom: 16, overflow: "hidden",
  }}>
    {(title || eyebrow) && (
      <header style={{ padding: "12px 16px", borderBottom: "1px solid var(--paper-line)", background: "var(--paper-deep)",
                       display: "flex", justifyContent: "space-between", alignItems: "center", gap: 12 }}>
        <div>
          {eyebrow && <div className="lb-eyebrow" style={{ marginBottom: 2 }}>{eyebrow}</div>}
          {title && <div className="lb-h2" style={{ fontSize: 14 }}>{title}</div>}
        </div>
        {action}
      </header>
    )}
    <div style={{ padding: "12px 16px" }}>{children}</div>
  </section>
);
const Code = ({ children }) => (
  <pre className="lb-mono" style={{
    margin: 0, padding: 12,
    background: "var(--paper-deep)", border: "1px solid var(--paper-line)", borderRadius: "var(--r-1)",
    fontSize: 12, lineHeight: 1.55, color: "var(--ink-deep)",
    overflowX: "auto", whiteSpace: "pre",
  }}>{children}</pre>
);

// Detail renderers — keep concise, pivot on object essentials.
function renderDetail(type, id) {
  const A = window.LIBRA_API;
  switch (type) {
    case "thread":      return <ThreadDetail t={A.thread}/>;
    case "intent":      return <IntentDetail in_={A.intent_head}/>;
    case "intentspec":  return <IntentSpecDetail spec={A.intent_head.spec}/>;
    case "plan": {
      const p = A.plans.find(x => x.plan_id === id) || A.plans[0];
      return <PlanDetail p={p}/>;
    }
    case "task": {
      const t = A.tasks.find(x => x.task_id === id) || A.tasks[0];
      return <TaskDetail t={t}/>;
    }
    case "run": {
      const r = A.runs.find(x => x.run_id === id) || A.runs[0];
      return <RunDetail r={r}/>;
    }
    case "patchset": {
      const p = A.patchsets.find(x => x.patchset_id === id) || A.patchsets[0];
      return <PatchSetDetail p={p}/>;
    }
    case "provenance":  return <ProvenanceDetail pv={A.provenance_head}/>;
    case "intent_event": {
      const e = A.intent_events.find(x => x.event_id === id) || A.intent_events[0];
      return <KvCard title="Intent event">
        <KV k="event_id"  v={e.event_id} mono/>
        <KV k="kind"      v={e.kind}/>
        <KV k="at"        v={fmtDateTime(e.at)}/>
        <KV k="actor"     v={e.actor} mono/>
        <KV k="intent_id" v={e.intent_id} mono/>
        {e.next_intent_id && <KV k="next_intent_id" v={e.next_intent_id} mono/>}
      </KvCard>;
    }
    case "tool_invocation": {
      const t = A.tool_invocations.find(x => x.invocation_id === id) || A.tool_invocations[0];
      return <KvCard title="Tool invocation">
        <KV k="invocation_id" v={t.invocation_id} mono/>
        <KV k="tool"          v={`${t.tool} · ${t.action}`}/>
        <KV k="run_id"        v={t.run_id} mono/>
        <KV k="at"            v={fmtDateTime(t.at)}/>
        <KV k="args"          v={t.args_summary} mono/>
        <KV k="exit_code"     v={t.exit_code}/>
        <KV k="paths_read"    v={(t.io_footprint.paths_read || []).join(", ") || "—"} mono/>
        <KV k="paths_written" v={(t.io_footprint.paths_written || []).join(", ") || "—"} mono/>
      </KvCard>;
    }
    case "evidence": {
      const e = A.evidence.find(x => x.evidence_id === id) || A.evidence[0];
      return <KvCard title="Evidence">
        <KV k="evidence_id" v={e.evidence_id} mono/>
        <KV k="kind"        v={e.kind}/>
        <KV k="status"      v={<StatusPill kind={e.status}/>}/>
        <KV k="run_id"      v={e.run_id} mono/>
        <KV k="at"          v={fmtDateTime(e.at)}/>
        <KV k="summary"     v={e.summary}/>
        <KV k="artifact"    v={e.artifact ? `${e.artifact.name} · ${e.artifact.format}` : "—"}/>
      </KvCard>;
    }
    case "decision": {
      const d = A.decisions.find(x => x.decision_id === id) || A.decisions[0];
      return <KvCard title="Decision">
        <KV k="decision_id" v={d.decision_id} mono/>
        <KV k="kind"        v={d.kind}/>
        <KV k="run_id"      v={d.run_id} mono/>
        <KV k="actor"       v={d.actor} mono/>
        <KV k="at"          v={fmtDateTime(d.at)}/>
        <KV k="rationale"   v={d.rationale}/>
        {d.chosen_patchset_id && <KV k="chosen_patchset_id" v={d.chosen_patchset_id} mono/>}
      </KvCard>;
    }
    case "context_frame": {
      const f = A.context_frames.find(x => x.frame_id === id) || A.context_frames[0];
      return <KvCard title="Context frame">
        <KV k="frame_id"  v={f.frame_id} mono/>
        <KV k="kind"      v={f.kind}/>
        <KV k="protected" v={f.protected ? "yes" : "no"}/>
        <KV k="trust"     v={f.trust}/>
        <KV k="at"        v={fmtDateTime(f.at)}/>
        <KV k="summary"   v={f.summary}/>
      </KvCard>;
    }
    case "run_usage": {
      const u = A.run_usage.find(x => x.usage_id === id) || A.run_usage[0];
      return <KvCard title="Run usage">
        <KV k="usage_id"           v={u.usage_id} mono/>
        <KV k="run_id"             v={u.run_id} mono/>
        <KV k="tokens_in"          v={u.tokens_in.toLocaleString()} mono/>
        <KV k="tokens_out"         v={u.tokens_out.toLocaleString()} mono/>
        <KV k="cost_usd"           v={`$${u.cost_usd.toFixed(4)}`} mono/>
        <KV k="wall_clock_seconds" v={`${u.wall_clock_seconds}s`} mono/>
      </KvCard>;
    }
    case "scheduler":   return <SchedulerDetail s={A.scheduler}/>;
    default: return <DetailEmpty type={type}/>;
  }
}

const KvCard = ({ title, children }) => <Card title={title} eyebrow="Object">{children}</Card>;

// ── Per-type detail components ─────────────────────────────────────────────
function ThreadDetail({ t }) {
  return (
    <>
      <Card eyebrow="Thread (Libra projection)" title={t.title}>
        <KV k="thread_id"         v={t.thread_id} mono/>
        <KV k="owner"             v={`${t.owner.display_name} · ${t.owner.type}/${t.owner.id}`}/>
        <KV k="current_intent_id" v={t.current_intent_id} mono/>
        <KV k="latest_intent_id"  v={t.latest_intent_id} mono/>
        <KV k="archived"          v={t.archived ? "yes" : "no"}/>
      </Card>
      <Card title="Participants">
        {t.participants.map(p => (
          <div key={p.id} style={{ display: "flex", justifyContent: "space-between", padding: "6px 0",
                                   borderBottom: "1px solid var(--paper-edge)" }}>
            <span className="lb-mono" style={{ fontSize: 12.5 }}>{p.type}/{p.id}</span>
            <span className="lb-meta" style={{ fontSize: 11.5 }}>{p.role} · joined {fmtRelative(p.joined_at)}</span>
          </div>
        ))}
      </Card>
      <Card title="Intent revisions">
        {t.intents.map(i => (
          <div key={i.intent_id} style={{ display: "flex", justifyContent: "space-between", padding: "6px 0",
                                           borderBottom: "1px solid var(--paper-edge)" }}>
            <span className="lb-mono" style={{ fontSize: 12.5, color: i.is_head ? "var(--ink-deep)" : "var(--ink-mid)", fontWeight: i.is_head ? 600 : 400 }}>
              #{i.ordinal} · {i.intent_id} {i.is_head && <span className="lb-chip" data-tone="info" style={{ height: 18, marginLeft: 6 }}>head</span>}
            </span>
            <span className="lb-meta" style={{ fontSize: 11.5 }}>{i.link_reason} · {fmtRelative(i.linked_at)}</span>
          </div>
        ))}
      </Card>
    </>
  );
}

function IntentDetail({ in_ }) {
  return (
    <>
      <Card eyebrow="Intent (snapshot)" title={in_.prompt}>
        <KV k="intent_id"  v={in_.intent_id} mono/>
        <KV k="parents"    v={(in_.parents || []).join(", ") || "—"} mono/>
        <KV k="created_at" v={fmtDateTime(in_.created_at)}/>
        <KV k="created_by" v={`${in_.created_by.type}/${in_.created_by.id}`} mono/>
        <KV k="status"     v={<StatusPill kind="proposed">{in_.status}</StatusPill>}/>
      </Card>
      <Card title="IntentSpec excerpt" action={<span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>{in_.spec.metadata.id}</span>}>
        <KV k="summary"     v={in_.spec.intent.summary}/>
        <KV k="change_type" v={in_.spec.intent.change_type}/>
        <KV k="risk"        v={`${in_.spec.risk.level} · approvers ≥ ${in_.spec.risk.human_in_loop.min_approvers}`}/>
        <KV k="in_scope"    v={in_.spec.intent.in_scope.join(", ")} mono/>
      </Card>
    </>
  );
}

function IntentSpecDetail({ spec }) {
  return (
    <>
      <Card eyebrow={spec.api_version} title={spec.intent.summary}>
        <KV k="id"          v={spec.metadata.id} mono/>
        <KV k="change_type" v={spec.intent.change_type}/>
        <KV k="risk"        v={`${spec.risk.level} · ${spec.risk.factors.join(", ")}`}/>
        <KV k="created_by"  v={`${spec.metadata.created_by.display_name} (${spec.metadata.created_by.id})`}/>
        <KV k="repo"        v={spec.metadata.target.repo.locator} mono/>
        <KV k="base_ref"    v={spec.metadata.target.base_ref} mono/>
      </Card>
      <Card title="Objectives">
        {spec.intent.objectives.map((o, i) => (
          <div key={i} style={{ display: "flex", justifyContent: "space-between", padding: "6px 0",
                                 borderBottom: "1px solid var(--paper-edge)" }}>
            <span style={{ fontSize: 12.5, color: "var(--ink-deep)" }}>{o.title}</span>
            <span className="lb-chip" style={{ height: 20 }}>{o.kind}</span>
          </div>
        ))}
      </Card>
      <Card title="Acceptance · success criteria">
        <ul style={{ margin: 0, paddingLeft: 18, fontSize: 12.5, color: "var(--ink-deep)" }}>
          {spec.acceptance.success_criteria.map((c, i) => <li key={i} style={{ marginBottom: 4 }}>{c}</li>)}
        </ul>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fit, minmax(140px, 1fr))", gap: 12, marginTop: 12 }}>
          {Object.entries(spec.acceptance.verification_plan).map(([k, v]) => (
            <div key={k}>
              <div className="lb-eyebrow" style={{ fontSize: 10 }}>{k.replace(/_/g, " ")}</div>
              <div className="lb-mono" style={{ fontSize: 14, color: "var(--ink-deep)" }}>{v}</div>
            </div>
          ))}
        </div>
      </Card>
      <Card title="Constraints">
        <KV k="security"  v={`network: ${spec.constraints.security.network_policy} · deps: ${spec.constraints.security.dependency_policy}`}/>
        <KV k="privacy"   v={`classes: ${spec.constraints.privacy.data_classes_allowed.join(", ")} · retention ${spec.constraints.privacy.retention_days}d`}/>
        <KV k="licensing" v={spec.constraints.licensing.allowed_spdx.join(", ")} mono/>
        <KV k="platform"  v={`${spec.constraints.platform.language_runtime} · ${spec.constraints.platform.supported_os.join(", ")}`}/>
        <KV k="resources" v={`≤${spec.constraints.resources.max_wall_clock_seconds}s · ≤${spec.constraints.resources.max_cost_units} units`}/>
      </Card>
      <Card title="Evidence policy">
        <KV k="strategy"        v={spec.evidence.strategy}/>
        <KV k="trust_tiers"     v={spec.evidence.trust_tiers.join(" → ")}/>
        <KV k="domain mode"     v={spec.evidence.domain_allowlist_mode}/>
        <KV k="allowed_domains" v={spec.evidence.allowed_domains.join(", ")} mono/>
        <KV k="min_citations"   v={spec.evidence.min_citations_per_decision}/>
      </Card>
      <Card title="Security · tool ACL">
        <div className="lb-eyebrow" style={{ marginBottom: 6 }}>Allow</div>
        <Code>{spec.security.tool_acl.allow.map(a => `${a.tool}: ${a.actions.join(",")}${a.constraints ? "  // " + JSON.stringify(a.constraints) : ""}`).join("\n")}</Code>
        <div className="lb-eyebrow" style={{ marginTop: 14, marginBottom: 6 }}>Deny</div>
        <Code>{spec.security.tool_acl.deny.map(a => `${a.tool}: ${a.actions.join(",")}${a.constraints ? "  // " + JSON.stringify(a.constraints) : ""}`).join("\n")}</Code>
      </Card>
      <Card title="Provenance">
        <KV k="SLSA"             v={spec.provenance.require_slsa_provenance ? "required" : "—"}/>
        <KV k="SBOM"             v={spec.provenance.require_sbom ? "required" : "—"}/>
        <KV k="transparency_log" v={spec.provenance.transparency_log.mode}/>
        <KV k="bindings"         v={Object.keys(spec.provenance.bindings).join(", ")}/>
      </Card>
      <Card title="Lifecycle change log">
        {spec.lifecycle.change_log.map((c, i) => (
          <div key={i} style={{ padding: "6px 0", borderBottom: "1px solid var(--paper-edge)" }}>
            <div style={{ display: "flex", justifyContent: "space-between" }}>
              <span className="lb-mono" style={{ fontSize: 12 }}>{fmtDateTime(c.at)} · {c.by}</span>
              <span className="lb-chip" style={{ height: 18 }}>{c.reason}</span>
            </div>
            <div className="lb-meta" style={{ fontSize: 12, marginTop: 2 }}>{c.diff_summary}</div>
          </div>
        ))}
      </Card>
    </>
  );
}

function PlanDetail({ p }) {
  return (
    <>
      <Card eyebrow={`Plan · ${p.kind}`} title={`${p.steps.length} steps`}>
        <KV k="plan_id"   v={p.plan_id} mono/>
        <KV k="intent_id" v={p.intent_id} mono/>
        <KV k="parents"   v={(p.parents || []).join(", ") || "—"} mono/>
        <KV k="created"   v={fmtDateTime(p.created_at)}/>
      </Card>
      <Card title="Steps">
        {p.steps.map(s => (
          <div key={s.step_id} style={{ display: "grid", gridTemplateColumns: "120px 1fr auto",
                                          gap: 12, padding: "8px 0", borderBottom: "1px solid var(--paper-edge)",
                                          alignItems: "center" }}>
            <span className="lb-mono" style={{ fontSize: 12 }}>{s.step_id}</span>
            <div>
              <div style={{ fontSize: 12.5, color: "var(--ink-deep)" }}>{s.title}</div>
              {s.depends_on.length > 0 && <div className="lb-meta" style={{ fontSize: 11 }}>after {s.depends_on.join(", ")}</div>}
            </div>
            <StatusPill kind={s.status}/>
          </div>
        ))}
      </Card>
    </>
  );
}

function TaskDetail({ t }) {
  return (
    <>
      <Card eyebrow="Task" title={t.title}>
        <KV k="task_id"        v={t.task_id} mono/>
        <KV k="origin_step"    v={t.origin_step_id} mono/>
        <KV k="intent_id"      v={t.intent_id} mono/>
        <KV k="goal"           v={t.goal}/>
        <KV k="status"         v={<StatusPill kind={t.status}/>}/>
        <KV k="run_count"      v={t.run_count}/>
        <KV k="dependencies"   v={t.dependencies.join(", ") || "—"} mono/>
      </Card>
      <Card title="Constraints">
        <ul style={{ margin: 0, paddingLeft: 18, fontSize: 12.5, color: "var(--ink-deep)" }}>
          {t.constraints.map((c, i) => <li key={i} style={{ marginBottom: 4 }}>{c}</li>)}
        </ul>
      </Card>
    </>
  );
}

function RunDetail({ r }) {
  const u = window.LIBRA_API.run_usage.find(x => x.run_id === r.run_id);
  return (
    <>
      <Card eyebrow="Run" title={r.run_id}>
        <KV k="task_id"     v={r.task_id} mono/>
        <KV k="plan_id"     v={r.plan_id} mono/>
        <KV k="commit"      v={shortSha(r.commit)} mono/>
        <KV k="started_at"  v={fmtDateTime(r.started_at)}/>
        <KV k="finished_at" v={r.finished_at ? fmtDateTime(r.finished_at) : "—"}/>
        <KV k="duration"    v={`${r.duration_seconds}s`}/>
        <KV k="status"      v={<StatusPill kind={r.status}/>}/>
        <KV k="retry_index" v={r.retry_index}/>
        <KV k="patchsets"   v={r.patchset_count}/>
      </Card>
      {u && (
        <Card title="RunUsage">
          <KV k="tokens_in"  v={u.tokens_in.toLocaleString()} mono/>
          <KV k="tokens_out" v={u.tokens_out.toLocaleString()} mono/>
          <KV k="cost_usd"   v={`$${u.cost_usd.toFixed(4)}`} mono/>
          <KV k="wall_clock" v={`${u.wall_clock_seconds}s`} mono/>
        </Card>
      )}
    </>
  );
}

function PatchSetDetail({ p }) {
  return (
    <>
      <Card eyebrow="PatchSet" title={p.patchset_id}>
        <KV k="run_id"     v={p.run_id} mono/>
        <KV k="commit"     v={shortSha(p.commit)} mono/>
        <KV k="format"     v={p.format}/>
        <KV k="touched"    v={p.touched.join(", ")} mono/>
        <KV k="lines"      v={<><span style={{ color: "var(--good)" }}>+{p.lines_added}</span> · <span style={{ color: "var(--bad)" }}>−{p.lines_removed}</span> · {p.hunks} hunks</>}/>
        <KV k="rationale"  v={p.rationale}/>
      </Card>
      <Card title="Diff (excerpt)">
        <Code>{`@@ src/ledger/journal.ts @@
- throw new Error('invalid');
+ throw new JournalError(JournalErrorCode.Validation, { errors });
@@
- const result = await writeEntry(args);
+ const result = await withRetry(() => writeEntry(args), {
+   idempotencyKey: args.idempotencyKey,
+ });`}</Code>
      </Card>
    </>
  );
}

function ProvenanceDetail({ pv }) {
  return (
    <>
      <Card eyebrow="Provenance" title={`${pv.model} · ${pv.provider}`}>
        <KV k="provenance_id"     v={pv.provenance_id} mono/>
        <KV k="run_id"            v={pv.run_id} mono/>
        <KV k="builder_id"        v={pv.builder_id} mono/>
        <KV k="slsa_level"        v={<StatusPill kind="good">{pv.slsa_level}</StatusPill>}/>
        <KV k="intentspec_digest" v={pv.intentspec_digest} mono/>
      </Card>
      <Card title="Parameters">
        {Object.entries(pv.parameters).map(([k, v]) => (
          <KV key={k} k={k} v={typeof v === "object" ? JSON.stringify(v) : String(v)} mono/>
        ))}
      </Card>
    </>
  );
}

function SchedulerDetail({ s }) {
  return (
    <>
      <Card eyebrow="Libra projection" title="Scheduler">
        <KV k="active_task_id"   v={s.active_task_id} mono/>
        <KV k="active_run_id"    v={s.active_run_id} mono/>
        <KV k="active_dag_stage" v={s.active_dag_stage}/>
        <KV k="ready_queue"      v={s.ready_queue.join(", ") || "—"} mono/>
      </Card>
      <Card title="Selected plans">
        <KV k="selected_plan_ids"   v={s.selected_plan_ids.join(", ")} mono/>
        <KV k="current_plan_heads"  v={s.current_plan_heads.join(", ")} mono/>
      </Card>
      <Card title="Live context window">
        <ul style={{ margin: 0, paddingLeft: 18, fontSize: 12.5, color: "var(--ink-deep)" }}>
          {s.live_context_window.map(f => <li key={f} className="lb-mono" style={{ marginBottom: 4 }}>{f}</li>)}
        </ul>
      </Card>
      <Card title="Parallel groups">
        {s.parallel_groups.map((g, i) => (
          <div key={i} className="lb-mono" style={{ fontSize: 12.5, padding: "4px 0",
                                                     borderBottom: "1px solid var(--paper-edge)" }}>
            [{g.join(", ")}]
          </div>
        ))}
      </Card>
    </>
  );
}

window.AiObjectsPage = AiObjectsPage;
