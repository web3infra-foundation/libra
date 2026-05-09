/* global React, AppShell, PageHeader, StatusPill, RefPicker, CopyButton,
   useHashRoute, useViewerMode, fmtRelative, shortSha, fmtBytes */
// Publish page — V1 hero. Recovery / clone entry point.
// Command shape: libra clone libra+cloud://<clone-domain>/<slug>[?ref=...|?revision=...]
// per docs/improvement/publish.md "Clone domain 方案" (line 273+).

const { useState, useMemo } = React;

function PublishPage() {
  const route = useHashRoute();
  const repo = window.LIBRA_API.repository;
  const refs = window.LIBRA_API.refs;
  const refName = route.params.ref || repo.default_branch;
  const revision = route.params.revision || "";
  const localPath = route.params.local || "";
  const setRef = (name) => route.updateParams({ ref: name === repo.default_branch ? null : name });

  const cur = refs.find(r => r.name === refName) || refs.find(r => r.is_default);

  // Variants of the clone command, all using the domain-qualified
  // libra+cloud://<clone-domain>/<slug> scheme. Falls back to a
  // placeholder clone-domain when the mock dataset doesn't carry
  // one yet (Phase 6+ wires the real value through).
  const slug = repo.slug;
  const cloneDomain = repo.clone_domain || "<clone-domain>";
  const repoId = repo.repo_id || repo.id || "<repo-id>";
  const cloneVariants = useMemo(() => {
    const base = `libra+cloud://${cloneDomain}/${slug}`;
    const stable = `libra+cloud://${cloneDomain}/repo/${repoId}`;
    return [
      {
        id: "default",
        title: "Default — clone HEAD of default branch",
        cmd: `libra clone ${base}${localPath ? " " + localPath : ""}`,
        notes: `Resolves to refs/heads/${repo.default_branch} @ ${shortSha(repo.head_sha)}`,
      },
      {
        id: "ref",
        title: "Pin to a branch or tag",
        cmd: `libra clone "${base}?ref=${cur?.name}"${localPath ? " " + localPath : ""}`,
        notes: cur?.type === "tag"
          ? `Tag · ${cur?.name} @ ${shortSha(cur?.oid)}`
          : `Branch · ${cur?.name} @ ${shortSha(cur?.oid)} · ${fmtRelative(cur?.last_commit_at)}`,
      },
      {
        id: "revision",
        title: "Pin to an immutable revision",
        cmd: `libra clone "${base}?revision=${revision || cur?.oid || "<oid>"}"${localPath ? " " + localPath : ""}`,
        notes: revision
          ? `Frozen at oid ${shortSha(revision)}`
          : `Use a full commit oid; the keyword \`latest\` resolves at clone time.`,
      },
      {
        id: "stable",
        title: "Stable repo URL — survives slug rename",
        cmd: `libra clone ${stable}${localPath ? " " + localPath : ""}`,
        notes: `Domain-qualified by repo_id; rename-proof.`,
      },
    ];
  }, [cloneDomain, slug, repoId, localPath, repo.default_branch, repo.head_sha, cur, revision]);

  return (
    <AppShell>
      <PageHeader
        eyebrow="Publish"
        title={repo.slug}
        subtitle={
          <>
            {repo.description} · <span className="lb-mono">{shortSha(repo.head_sha)}</span> on{" "}
            <span className="lb-mono">{repo.default_branch}</span> · {fmtBytes(repo.storage_bytes)} ·{" "}
            <StatusPill kind={repo.sync_state} />
          </>
        }
        refPicker={<RefPicker refs={refs} value={refName} onChange={setRef} />}
      />

      <div style={{
        flex: 1,
        overflow: "auto",
        padding: "clamp(16px, 3vw, 28px)",
        paddingBottom: 64,
        display: "flex",
        flexDirection: "column",
        gap: "clamp(20px, 3vw, 32px)",
        maxWidth: "var(--content-max)",
        width: "100%",
        boxSizing: "border-box",
      }}>
        <CloneSection variants={cloneVariants} cur={cur} />
        <QuickTaskGrid refName={refName} />
        <RefsTable refs={refs} active={refName} onPick={setRef} />
      </div>
    </AppShell>
  );
}

// ── Clone command block ────────────────────────────────────────────────────
function CloneSection({ variants, cur }) {
  const [tab, setTab] = useState("default");
  const cur_v = variants.find(v => v.id === tab);

  return (
    <section aria-labelledby="clone-h" style={{
      background: "var(--paper-deep)",
      border: "1px solid var(--paper-line)",
      borderRadius: "var(--r-2)",
      overflow: "hidden",
    }}>
      <header style={{
        display: "flex",
        alignItems: "flex-start",
        justifyContent: "space-between",
        gap: 16,
        padding: "18px clamp(16px, 3vw, 24px) 14px",
        borderBottom: "1px solid var(--paper-line)",
        background: "var(--paper)",
        flexWrap: "wrap",
      }}>
        <div style={{ minWidth: 0, flex: "1 1 280px" }}>
          <div className="lb-eyebrow" style={{ marginBottom: 4 }}>Recovery / clone</div>
          <h2 id="clone-h" className="lb-h2" style={{ fontSize: 20 }}>
            Restore this repository with the Libra CLI
          </h2>
          <p className="lb-meta" style={{ marginTop: 6, maxWidth: 640 }}>
            This page exposes publish metadata and the clone command. The CLI uses your local
            Cloudflare/Libra configuration to read the published code and AI object model directly
            from D1 and R2 — no Worker download or auth flow runs on this page.
          </p>
        </div>
        <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-end", gap: 6 }}>
          <span className="lb-eyebrow" style={{ fontSize: 10 }}>Selected ref</span>
          <span className="lb-mono" style={{ fontSize: 13.5, fontWeight: 600, color: "var(--ink-deep)" }}>
            {cur?.name}
          </span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>
            {shortSha(cur?.oid)}{cur?.type === "tag" ? " · tag" : ""}
          </span>
        </div>
      </header>

      <div style={{
        display: "flex",
        gap: 0,
        padding: "0 clamp(16px, 3vw, 24px)",
        borderBottom: "1px solid var(--paper-line)",
        background: "var(--paper)",
        overflowX: "auto",
      }}>
        <div role="tablist" aria-label="Clone variant" style={{ display: "flex", gap: 0 }}>
          {variants.map(v => {
            const on = tab === v.id;
            return (
              <button
                key={v.id}
                role="tab"
                aria-selected={on}
                onClick={() => setTab(v.id)}
                style={{
                  padding: "12px 16px",
                  fontFamily: "var(--sans)",
                  fontSize: 12.5,
                  fontWeight: on ? 600 : 500,
                  color: on ? "var(--ink-deep)" : "var(--ink-soft)",
                  borderBottom: on ? "2px solid var(--gold)" : "2px solid transparent",
                  marginBottom: -1,
                  whiteSpace: "nowrap",
                }}
              >{v.title}</button>
            );
          })}
        </div>
      </div>

      <div style={{ padding: "16px clamp(16px, 3vw, 24px) 18px" }}>
        <CommandLine value={cur_v.cmd} copyKey={`clone-${cur_v.id}`} />
        <div className="lb-meta" style={{ marginTop: 10, fontSize: 12 }}>
          {cur_v.notes}
        </div>
        <div style={{
          marginTop: 18,
          paddingTop: 16,
          borderTop: "1px solid var(--paper-line)",
          display: "grid",
          gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))",
          gap: 18,
        }}>
          <Fact k="HEAD oid"        v={shortSha(cur?.oid)} mono />
          <Fact k="Selected ref"    v={cur?.name} mono />
          <Fact k="Last commit"     v={`${fmtRelative(cur?.last_commit_at)} · ${cur?.last_commit_author}`} />
          <Fact k="Publish state"   v={<StatusPill kind={cur?.publish_state} />} />
        </div>
      </div>
    </section>
  );
}

function CommandLine({ value, copyKey }) {
  return (
    <div style={{
      display: "flex",
      alignItems: "stretch",
      flexWrap: "wrap",
      border: "1px solid var(--ink)",
      borderRadius: "var(--r-2)",
      background: "var(--paper)",
      overflow: "hidden",
    }}>
      <pre className="lb-mono" style={{
        margin: 0,
        padding: "14px 16px",
        flex: "1 1 280px",
        minWidth: 0,
        fontSize: 13.5,
        color: "var(--ink-deep)",
        overflowX: "auto",
        whiteSpace: "pre",
      }}>
        <span style={{ color: "var(--ink-faint)", userSelect: "none" }}>$ </span>{value}
      </pre>
      <div style={{
        display: "flex",
        alignItems: "center",
        padding: "8px 10px",
        borderLeft: "1px solid var(--paper-line)",
        background: "var(--paper-deep)",
        flex: "0 0 auto",
        width: "100%",
        justifyContent: "flex-end",
      }}
      data-cmd-actions
      >
        <CopyButton value={value} label="Copy" copyKey={copyKey}/>
      </div>
    </div>
  );
}

function Fact({ k, v, mono }) {
  return (
    <div style={{ minWidth: 0 }}>
      <div className="lb-eyebrow" style={{ marginBottom: 4 }}>{k}</div>
      <div className={mono ? "lb-mono" : ""} style={{
        fontSize: 13,
        color: "var(--ink-deep)",
        overflow: "hidden",
        textOverflow: "ellipsis",
      }}>
        {v}
      </div>
    </div>
  );
}

// ── Quick-task grid (deep links) ────────────────────────────────────────────
function QuickTaskGrid({ refName }) {
  const route = useHashRoute();
  const items = [
    { eyebrow: "Browse",     title: "Code & directory tree",     desc: "Explore files, language stats, redacted regions.",
      goto: () => route.navigate("code", [], { ref: refName }) },
    { eyebrow: "Inspect",    title: "AI object model",           desc: "Snapshots, events, and Libra projections.",
      goto: () => route.navigate("ai", [], { ref: refName, type: "snapshot" }) },
    { eyebrow: "Track",      title: "Refs & publish state",      desc: "All branches and tags · publish health.",
      goto: () => route.navigate("publish", [], { ref: refName }), variant: "soft" },
  ];
  const visible = items;

  return (
    <section aria-label="Quick tasks">
      <h2 className="lb-h2" style={{ marginBottom: 12 }}>What do you need to do?</h2>
      <div style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(240px, 1fr))",
        gap: 14,
      }}>
        {visible.map((it, i) => (
          <button
            key={i}
            type="button"
            onClick={it.goto}
            style={{
              border: "1px solid var(--paper-line)",
              borderRadius: "var(--r-2)",
              background: "var(--paper)",
              padding: "16px 18px",
              display: "flex",
              flexDirection: "column",
              gap: 6,
              textAlign: "left",
              transition: "border-color 0.15s, box-shadow 0.15s, background 0.15s",
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.borderColor = "var(--ink-mid)";
              e.currentTarget.style.boxShadow = "var(--shadow-1)";
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.borderColor = "var(--paper-line)";
              e.currentTarget.style.boxShadow = "none";
            }}
          >
            <span className="lb-eyebrow">{it.eyebrow}</span>
            <span className="lb-h2" style={{ fontSize: 15 }}>{it.title}</span>
            <span className="lb-meta" style={{ fontSize: 12.5, marginTop: 2 }}>{it.desc}</span>
            <span style={{
              marginTop: 8, color: "var(--gold)",
              fontSize: 12, fontWeight: 600, fontFamily: "var(--sans)",
              display: "inline-flex", alignItems: "center", gap: 4,
            }}>Open <span aria-hidden="true">→</span></span>
          </button>
        ))}
      </div>
    </section>
  );
}

// ── All-refs publish status table ──────────────────────────────────────────
function RefsTable({ refs, active, onPick }) {
  const [tab, setTab] = useState("all");
  const filtered = refs.filter(r => tab === "all" || r.type === tab);
  const counts = {
    all:    refs.length,
    branch: refs.filter(r => r.type === "branch").length,
    tag:    refs.filter(r => r.type === "tag").length,
  };

  return (
    <section aria-labelledby="refs-h" style={{
      background: "var(--paper)",
      border: "1px solid var(--paper-line)",
      borderRadius: "var(--r-2)",
      overflow: "hidden",
    }}>
      <header style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: 12,
        padding: "14px clamp(12px, 2vw, 18px)",
        borderBottom: "1px solid var(--paper-line)",
        flexWrap: "wrap",
      }}>
        <div>
          <div className="lb-eyebrow" style={{ marginBottom: 2 }}>All refs</div>
          <h2 id="refs-h" className="lb-h2" style={{ fontSize: 16 }}>Branches & tags · publish status</h2>
        </div>
        <div role="tablist" aria-label="Ref filter" style={{
          display: "flex", border: "1px solid var(--paper-line)", borderRadius: "var(--r-1)", overflow: "hidden",
        }}>
          {[
            { id: "all",    label: "All" },
            { id: "branch", label: "Branches" },
            { id: "tag",    label: "Tags" },
          ].map(t => {
            const on = tab === t.id;
            return (
              <button
                key={t.id} role="tab" aria-selected={on}
                onClick={() => setTab(t.id)}
                style={{
                  padding: "6px 12px",
                  background: on ? "var(--ink-deep)" : "var(--paper)",
                  color: on ? "var(--paper)" : "var(--ink-mid)",
                  fontFamily: "var(--sans)", fontSize: 11.5, fontWeight: 600,
                }}
              >{t.label} <span className="lb-mono" style={{ opacity: 0.7, marginLeft: 4 }}>{counts[t.id]}</span></button>
            );
          })}
        </div>
      </header>

      {/* Table — horizontal scroll on small screens */}
      <div style={{ overflowX: "auto" }}>
        <div role="table" aria-label="Refs" style={{
          minWidth: 760,
          display: "grid",
          gridTemplateColumns: "minmax(180px, 1.6fr) 96px 110px 130px 1fr 130px 110px",
          fontFamily: "var(--sans)",
        }}>
          <RefHeaderCell>Name</RefHeaderCell>
          <RefHeaderCell>Type</RefHeaderCell>
          <RefHeaderCell>OID</RefHeaderCell>
          <RefHeaderCell align="right">Ahead / behind</RefHeaderCell>
          <RefHeaderCell>Last commit</RefHeaderCell>
          <RefHeaderCell>Publish</RefHeaderCell>
          <RefHeaderCell align="right">AI versions</RefHeaderCell>

          {filtered.map(r => (
            <RefRow key={r.name} r={r} active={r.name === active} onPick={onPick} />
          ))}
        </div>
      </div>
    </section>
  );
}

function RefHeaderCell({ children, align }) {
  return (
    <div role="columnheader" className="lb-eyebrow" style={{
      padding: "10px 12px",
      borderBottom: "1px solid var(--paper-line)",
      background: "var(--paper-deep)",
      textAlign: align || "left",
      fontSize: 10.5,
    }}>{children}</div>
  );
}

function RefRow({ r, active, onPick }) {
  return (
    <>
      <div role="cell" style={{
        padding: "12px 12px",
        borderBottom: "1px solid var(--paper-edge)",
        background: active ? "var(--paper-deep)" : "transparent",
        borderLeft: active ? "2px solid var(--gold)" : "2px solid transparent",
        display: "flex", alignItems: "center", gap: 8, minWidth: 0,
      }}>
        <button type="button" onClick={() => onPick(r.name)}
          style={{
            fontFamily: "var(--mono)", fontSize: 12.5, fontWeight: active ? 600 : 500,
            color: "var(--ink-deep)",
            overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0,
          }}>
          {r.name}
        </button>
        {r.is_default && <span className="lb-chip" style={{ height: 18 }} data-tone="info">default</span>}
        {r.protected && <span className="lb-chip" style={{ height: 18 }}>protected</span>}
      </div>
      <div role="cell" style={cellStyle(active)}>
        <span className="lb-eyebrow" style={{ fontSize: 10 }}>{r.type}</span>
      </div>
      <div role="cell" style={cellStyle(active)}>
        <span className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink-mid)" }}>{shortSha(r.oid)}</span>
      </div>
      <div role="cell" style={{ ...cellStyle(active), textAlign: "right", fontFamily: "var(--mono)", fontSize: 11.5 }}>
        {r.ahead != null ? (
          <>
            <span style={{ color: "var(--good)" }}>↑{r.ahead}</span>
            <span style={{ color: "var(--ink-faint)" }}> / </span>
            <span style={{ color: "var(--warn)" }}>↓{r.behind}</span>
          </>
        ) : <span style={{ color: "var(--ink-faint)" }}>—</span>}
      </div>
      <div role="cell" style={{ ...cellStyle(active), minWidth: 0 }}>
        <div style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
                      fontSize: 12.5, color: "var(--ink)" }}>
          {r.last_commit_summary}
        </div>
        <div className="lb-meta" style={{ fontSize: 10.5, marginTop: 2 }}>
          {fmtRelative(r.last_commit_at)} · {r.last_commit_author}
        </div>
      </div>
      <div role="cell" style={cellStyle(active)}>
        <StatusPill kind={r.publish_state} />
      </div>
      <div role="cell" style={{ ...cellStyle(active), textAlign: "right", fontFamily: "var(--mono)", fontSize: 12.5,
                                color: r.ai_versions_count > 0 ? "var(--ink-deep)" : "var(--ink-faint)" }}>
        {r.ai_versions_count}
      </div>
    </>
  );
}

const cellStyle = (active) => ({
  padding: "12px 12px",
  borderBottom: "1px solid var(--paper-edge)",
  background: active ? "var(--paper-deep)" : "transparent",
  display: "flex", flexDirection: "column", justifyContent: "center",
});

window.PublishPage = PublishPage;
