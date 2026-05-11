/* global React, AppShell, PageHeader, RefPicker, Breadcrumb, StatusPill,
   useHashRoute, useViewerMode, useBreakpoint, fmtBytes, fmtRelative, shortSha */
// Code browser — ref picker, breadcrumb, dir/blob list, search, h-scroll viewer.
// Mobile: segmented view (tree | viewer).

const { useState, useMemo } = React;

function CodePage() {
  const route = useHashRoute();
  const refs = window.LIBRA_API.refs;
  const refName = route.params.ref || window.LIBRA_API.repository.default_branch;
  const setRef = (n) => route.updateParams({ ref: n === window.LIBRA_API.repository.default_branch ? null : n });

  const path = route.params.path || "";   // selected blob, or empty for tree
  const dir  = route.params.dir  || "";   // current tree directory

  const tree = window.LIBRA_API.file_tree;
  const cur = refs.find(r => r.name === refName) || refs[0];

  const bp = useBreakpoint();
  const [mobileTab, setMobileTab] = useState(path ? "viewer" : "tree");
  React.useEffect(() => { if (path) setMobileTab("viewer"); }, [path]);

  const segs = (dir ? dir.split("/") : []).filter(Boolean);
  const breadcrumb = (
    <Breadcrumb segments={[
      { label: window.LIBRA_API.repository.slug, onClick: () => route.updateParams({ dir: null, path: null }) },
      ...segs.map((s, i) => ({
        label: s,
        onClick: () => route.updateParams({ dir: segs.slice(0, i + 1).join("/"), path: null }),
      })),
    ]}/>
  );

  return (
    <AppShell>
      <PageHeader
        eyebrow="Code"
        title="Browse files"
        subtitle={<>Ref <span className="lb-mono">{cur.name}</span> · HEAD <span className="lb-mono">{shortSha(cur.oid)}</span> · {fmtRelative(cur.last_commit_at)}</>}
        breadcrumb={breadcrumb}
        refPicker={<RefPicker refs={refs} value={refName} onChange={setRef} />}
      />
      {bp === "mobile" && (
        <div role="tablist" aria-label="View" style={{
          display: "grid", gridTemplateColumns: "1fr 1fr",
          borderBottom: "1px solid var(--paper-line)", background: "var(--paper)",
        }}>
          {[{id:"tree",label:"Tree"},{id:"viewer",label:"File"}].map(t => {
            const on = mobileTab === t.id;
            return (
              <button key={t.id} role="tab" aria-selected={on}
                onClick={() => setMobileTab(t.id)}
                style={{
                  padding: "10px",
                  background: on ? "var(--paper-deep)" : "var(--paper)",
                  borderBottom: on ? "2px solid var(--gold)" : "2px solid transparent",
                  fontFamily: "var(--sans)", fontSize: 12.5, fontWeight: on ? 600 : 500,
                  color: on ? "var(--ink-deep)" : "var(--ink-soft)",
                }}>{t.label}</button>
            );
          })}
        </div>
      )}
      <div style={{
        flex: 1, minHeight: 0, display: "flex",
        flexDirection: bp === "mobile" ? "column" : "row",
      }}>
        {(bp !== "mobile" || mobileTab === "tree") && (
          <TreePanel tree={tree} dir={dir} path={path}
            onPickDir={(d) => route.updateParams({ dir: d, path: null })}
            onPickFile={(p) => { route.updateParams({ path: p }); if (bp === "mobile") setMobileTab("viewer"); }}
          />
        )}
        {(bp !== "mobile" || mobileTab === "viewer") && (
          <FileViewerPanel path={path} refName={refName} />
        )}
      </div>
    </AppShell>
  );
}

// ── Tree panel ──────────────────────────────────────────────────────────────
function TreePanel({ tree, dir, path, onPickDir, onPickFile }) {
  const [q, setQ] = useState("");
  const bp = useBreakpoint();

  // Filter rows: direct children of `dir` unless searching.
  const rows = useMemo(() => {
    if (q.trim()) {
      const t = q.toLowerCase();
      return tree.filter(e => e.path.toLowerCase().includes(t)).slice(0, 80);
    }
    const prefix = dir ? dir + "/" : "";
    return tree.filter(e => {
      if (!e.path.startsWith(prefix)) return false;
      const rest = e.path.slice(prefix.length);
      return rest && !rest.includes("/");
    });
  }, [tree, dir, q]);

  return (
    <section aria-label="File tree" style={{
      width: bp === "mobile" ? "100%" : "clamp(280px, 30%, 360px)",
      borderRight: bp === "mobile" ? "0" : "1px solid var(--paper-line)",
      borderBottom: bp === "mobile" ? "1px solid var(--paper-line)" : "0",
      display: "flex", flexDirection: "column", minHeight: 0,
      background: "var(--paper-deep)",
    }}>
      <div style={{ padding: "12px 14px", borderBottom: "1px solid var(--paper-line)" }}>
        <input
          type="search"
          value={q}
          onChange={e => setQ(e.target.value)}
          placeholder="Filter files…"
          aria-label="Filter files"
          style={{
            width: "100%", padding: "7px 10px",
            border: "1px solid var(--paper-line)", borderRadius: "var(--r-1)",
            background: "var(--paper)", fontFamily: "var(--mono)", fontSize: 12.5,
            color: "var(--ink-deep)",
          }}
        />
      </div>
      <div style={{ flex: 1, overflowY: "auto" }}>
        {dir && !q && (
          <button type="button"
            onClick={() => onPickDir(dir.split("/").slice(0, -1).join("/"))}
            style={{
              width: "100%", display: "flex", alignItems: "center", gap: 10,
              padding: "8px 14px", borderBottom: "1px solid var(--paper-edge)",
              fontFamily: "var(--mono)", fontSize: 12.5, color: "var(--ink-mid)",
            }}>
            <span aria-hidden="true">↑</span> ..
          </button>
        )}
        {rows.length === 0 && (
          <div className="lb-meta" style={{ padding: 24, textAlign: "center" }}>No matches.</div>
        )}
        {rows.map(e => {
          const name = q ? e.path : e.path.split("/").pop();
          const isSel = e.path === path;
          return (
            <button key={e.path}
              type="button"
              onClick={() => e.type === "tree" ? onPickDir(e.path) : onPickFile(e.path)}
              aria-current={isSel ? "true" : undefined}
              style={{
                width: "100%",
                display: "grid",
                gridTemplateColumns: "16px 1fr auto",
                alignItems: "center", gap: 10,
                padding: "8px 14px",
                borderBottom: "1px solid var(--paper-edge)",
                background: isSel ? "var(--paper)" : "transparent",
                borderLeft: isSel ? "2px solid var(--gold)" : "2px solid transparent",
                opacity: e.is_ignored ? 0.55 : 1,
              }}>
              <span aria-hidden="true" style={{ color: e.type === "tree" ? "var(--ink-mid)" : "var(--ink-faint)" }}>
                {e.type === "tree" ? "▸" : "·"}
              </span>
              <span className="lb-mono" style={{
                fontSize: 12.5,
                color: "var(--ink-deep)",
                fontWeight: e.type === "tree" ? 600 : 500,
                overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", textAlign: "left",
              }}>{name}</span>
              <span style={{ display: "flex", gap: 4, alignItems: "center" }}>
                {e.has_redactions && <span className="lb-chip" style={{ height: 18 }} data-tone="warn">redact</span>}
                {e.is_binary && <span className="lb-chip" style={{ height: 18 }}>bin</span>}
                {e.is_too_large && <span className="lb-chip" style={{ height: 18 }} data-tone="warn">large</span>}
                {e.is_ignored && <span className="lb-chip" style={{ height: 18 }}>ignored</span>}
                <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)", minWidth: 48, textAlign: "right" }}>
                  {e.type === "tree" ? `${e.file_count}f` : fmtBytes(e.size_bytes)}
                </span>
              </span>
            </button>
          );
        })}
      </div>
    </section>
  );
}

// ── File viewer panel ──────────────────────────────────────────────────────
function FileViewerPanel({ path, refName }) {
  const [tab, setTab] = useState("source"); // source | ai
  if (!path) return <ViewerEmpty />;
  const file = window.LIBRA_API.file_object__journal; // single mock
  const isJournal = path === file.path;

  // Error states for non-journal mock files
  if (!isJournal) {
    const e = window.LIBRA_API.file_tree.find(x => x.path === path);
    if (e?.is_binary)    return <ViewerError code="BINARY_NOT_VIEWABLE" path={path} hint="Binary content cannot be displayed inline. Use the CLI to fetch this blob."/>;
    if (e?.is_too_large) return <ViewerError code="BLOB_TOO_LARGE" path={path} hint={`Blob is ${fmtBytes(e.size_bytes)}; viewer limit is 2 MB.`}/>;
    if (e?.type === "blob") return <ViewerStub path={path} entry={e} />;
    return <ViewerEmpty />;
  }

  return (
    <section aria-label="File viewer" style={{
      flex: 1, minWidth: 0, minHeight: 0, display: "flex", flexDirection: "column",
    }}>
      <header style={{
        display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12,
        padding: "10px 16px", borderBottom: "1px solid var(--paper-line)",
        background: "var(--paper-deep)", flexWrap: "wrap",
      }}>
        <div style={{ minWidth: 0 }}>
          <div className="lb-mono" style={{ fontSize: 13, fontWeight: 600, color: "var(--ink-deep)",
                                            overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {file.path}
          </div>
          <div className="lb-meta" style={{ fontSize: 11, marginTop: 2 }}>
            {file.language} · {file.line_count} lines · {fmtBytes(file.size_bytes)} · blob {shortSha(file.blob_sha)} · {fmtRelative(file.last_modified_at)} · {file.last_author}
            {file.has_redactions && <> · <span style={{ color: "var(--warn)" }}>{file.redaction_count} redacted regions</span></>}
          </div>
        </div>
        <div role="tablist" aria-label="Viewer mode" style={{
          display: "flex", border: "1px solid var(--paper-line)", borderRadius: "var(--r-1)", overflow: "hidden",
        }}>
          {[{id:"source",label:"Source"},{id:"ai",label:`AI versions · ${window.LIBRA_API.ai_versions.length}`}].map(t => {
            const on = tab === t.id;
            return (
              <button key={t.id} role="tab" aria-selected={on}
                onClick={() => setTab(t.id)}
                style={{
                  padding: "6px 12px",
                  background: on ? "var(--ink-deep)" : "var(--paper)",
                  color: on ? "var(--paper)" : "var(--ink-mid)",
                  fontFamily: "var(--sans)", fontSize: 11.5, fontWeight: 600,
                }}>{t.label}</button>
            );
          })}
        </div>
      </header>
      <div style={{ flex: 1, minHeight: 0, overflow: "auto" }}>
        {tab === "source" ? <SourceView file={file}/> : <AiVersionsTab file={file}/>}
      </div>
    </section>
  );
}

function SourceView({ file }) {
  // Synthetic source lines with one redacted region (lines 11–13).
  const SAMPLE = [
    `import { JournalError, JournalErrorCode } from './errors';`,
    `import { withRetry } from '../runtime/retry';`,
    `import type { Posting, Entry } from './types';`,
    ``,
    `export interface PostJournalEntryArgs {`,
    `  entry: Entry;`,
    `  idempotencyKey?: string;`,
    `}`,
    ``,
    `export async function postJournalEntry(args: PostJournalEntryArgs) {`,
    `  const apiKey = process.env.LEDGER_API_KEY;`,
    `  const signingSecret = process.env.LEDGER_SIGNING_SECRET;`,
    `  const callbackToken = process.env.LEDGER_CALLBACK_TOKEN;`,
    ``,
    `  const errors = validate(args.entry);`,
    `  if (errors.length) {`,
    `    throw new JournalError(JournalErrorCode.Validation, { errors });`,
    `  }`,
    ``,
    `  return withRetry(() => writeEntry(args), { idempotencyKey: args.idempotencyKey });`,
    `}`,
  ];
  const REDACTED = new Set([10, 11, 12]); // 0-indexed

  return (
    <div style={{ display: "flex", fontFamily: "var(--mono)", fontSize: 12.5, lineHeight: 1.6,
                  background: "var(--paper)", minHeight: "100%" }}>
      <pre aria-hidden="true" style={{
        margin: 0, padding: "12px 14px",
        color: "var(--ink-faint)", textAlign: "right", userSelect: "none",
        borderRight: "1px solid var(--paper-line)", background: "var(--paper-deep)",
        fontVariantNumeric: "tabular-nums",
      }}>
        {SAMPLE.map((_, i) => i + 1).join("\n")}
      </pre>
      <div style={{ flex: 1, minWidth: 0, overflowX: "auto" }}>
        <pre style={{ margin: 0, padding: "12px 16px", color: "var(--ink-deep)", whiteSpace: "pre" }}>
          {SAMPLE.map((ln, i) => (
            <div key={i} style={{
              background: REDACTED.has(i) ? "var(--warn-tint)" : "transparent",
              padding: "0 4px", borderRadius: 2,
            }}>
              {REDACTED.has(i) ? <span style={{ color: "var(--warn)", fontStyle: "italic" }}>// ▒▒▒ redacted (env secret) ▒▒▒</span> : ln || " "}
            </div>
          ))}
        </pre>
      </div>
    </div>
  );
}

// ── AI Versions tab inside the file viewer ─────────────────────────────────
function AiVersionsTab({ file }) {
  const versions = window.LIBRA_API.ai_versions;
  const [picked, setPicked] = useState(versions[0].version_id);
  const v = versions.find(x => x.version_id === picked);
  const bp = useBreakpoint();

  return (
    <div style={{ display: "flex", flexDirection: bp === "mobile" ? "column" : "row", minHeight: "100%" }}>
      <aside aria-label="AI versions" style={{
        width: bp === "mobile" ? "100%" : 280,
        borderRight: bp === "mobile" ? "0" : "1px solid var(--paper-line)",
        borderBottom: bp === "mobile" ? "1px solid var(--paper-line)" : "0",
        background: "var(--paper-deep)",
      }}>
        {versions.map(x => {
          const on = x.version_id === picked;
          return (
            <button key={x.version_id} type="button" onClick={() => setPicked(x.version_id)}
              aria-current={on ? "true" : undefined}
              style={{
                display: "block", width: "100%",
                padding: "10px 14px",
                borderBottom: "1px solid var(--paper-edge)",
                background: on ? "var(--paper)" : "transparent",
                borderLeft: on ? "2px solid var(--gold)" : "2px solid transparent",
                textAlign: "left",
              }}>
              <div style={{ display: "flex", justifyContent: "space-between", gap: 8, alignItems: "center" }}>
                <span className="lb-mono" style={{ fontSize: 12, color: "var(--ink-deep)", fontWeight: on ? 600 : 500 }}>
                  {x.version_id}
                </span>
                <StatusPill kind={x.status}/>
              </div>
              <div className="lb-meta" style={{ fontSize: 11, marginTop: 4, lineHeight: 1.4 }}>
                {x.prompt_excerpt}
              </div>
              <div className="lb-mono" style={{ fontSize: 10.5, marginTop: 4, color: "var(--ink-soft)" }}>
                <span style={{ color: "var(--good)" }}>+{x.diff_stats.lines_added}</span>
                {" "}<span style={{ color: "var(--bad)" }}>−{x.diff_stats.lines_removed}</span>
                {" · "}{fmtRelative(x.created_at)}
              </div>
            </button>
          );
        })}
      </aside>
      <div style={{ flex: 1, minWidth: 0, padding: 16, display: "flex", flexDirection: "column", gap: 12 }}>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 12, alignItems: "baseline" }}>
          <h3 className="lb-h2" style={{ fontSize: 14 }}>Side-by-side · base {shortSha(v.base_blob_sha)} → {v.version_id}</h3>
          <span className="lb-meta" style={{ fontSize: 12 }}>
            {v.diff_stats.hunks} hunks · model {v.model} · confidence {(v.confidence*100).toFixed(0)}%
          </span>
        </div>
        <SideBySideDiff/>
      </div>
    </div>
  );
}

function SideBySideDiff() {
  const before = [
    { ln: 14, t: `if (errors.length) {` },
    { ln: 15, t: `  throw new Error('invalid');`, kind: "del" },
    { ln: 16, t: `}` },
    { ln: 17, t: `` },
  ];
  const after = [
    { ln: 14, t: `if (errors.length) {` },
    { ln: 15, t: `  throw new JournalError(JournalErrorCode.Validation, { errors });`, kind: "add" },
    { ln: 16, t: `}` },
    { ln: 17, t: `` },
  ];

  const Pane = ({ side, rows }) => (
    <div style={{ flex: "1 1 380px", minWidth: 0, border: "1px solid var(--paper-line)", borderRadius: "var(--r-1)",
                  background: "var(--paper)", overflow: "hidden", display: "flex", flexDirection: "column" }}>
      <header style={{ padding: "8px 12px", borderBottom: "1px solid var(--paper-line)",
                       background: "var(--paper-deep)", display: "flex", justifyContent: "space-between" }}>
        <span className="lb-eyebrow" style={{ fontSize: 10 }}>{side}</span>
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>journal.ts</span>
      </header>
      <div style={{ overflowX: "auto" }}>
        <pre className="lb-mono" style={{ margin: 0, fontSize: 12, lineHeight: 1.6, whiteSpace: "pre", minWidth: "fit-content" }}>
          {rows.map((r, i) => {
            const bg = r.kind === "add" ? "var(--good-tint)" : r.kind === "del" ? "var(--bad-tint)" : "transparent";
            const fg = r.kind === "add" ? "var(--good)" : r.kind === "del" ? "var(--bad)" : "var(--ink-deep)";
            const sign = r.kind === "add" ? "+" : r.kind === "del" ? "−" : " ";
            return (
              <div key={i} style={{ display: "flex", background: bg }}>
                <span style={{ width: 36, padding: "0 8px", textAlign: "right", color: "var(--ink-faint)",
                               borderRight: "1px solid var(--paper-edge)" }}>{r.ln}</span>
                <span style={{ width: 18, color: fg, textAlign: "center" }}>{sign}</span>
                <span style={{ padding: "0 10px", color: fg }}>{r.t || " "}</span>
              </div>
            );
          })}
        </pre>
      </div>
    </div>
  );

  return (
    <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
      <Pane side="Before" rows={before}/>
      <Pane side="After"  rows={after}/>
    </div>
  );
}

// ── Empty / Error / Stub ───────────────────────────────────────────────────
function ViewerEmpty() {
  return (
    <section style={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", padding: 32 }}>
      <div style={{ textAlign: "center", maxWidth: 320 }}>
        <div className="lb-eyebrow" style={{ marginBottom: 6 }}>No file selected</div>
        <p className="lb-meta">Pick any blob from the tree to view source, redactions, and AI versions.</p>
      </div>
    </section>
  );
}
function ViewerError({ code, path, hint }) {
  return (
    <section style={{ flex: 1, padding: 32 }}>
      <div className="lb-chip" data-tone="bad" style={{ marginBottom: 10 }}>{code}</div>
      <div className="lb-mono" style={{ fontSize: 13, color: "var(--ink-deep)", marginBottom: 6 }}>{path}</div>
      <p className="lb-meta">{hint}</p>
    </section>
  );
}
function ViewerStub({ path, entry }) {
  return (
    <section style={{ flex: 1, padding: 24 }}>
      <div className="lb-mono" style={{ fontSize: 13, color: "var(--ink-deep)", marginBottom: 6 }}>{path}</div>
      <p className="lb-meta">Source preview not generated for this fixture · {fmtBytes(entry.size_bytes)} · {entry.language || "—"}.</p>
    </section>
  );
}

window.CodePage = CodePage;
