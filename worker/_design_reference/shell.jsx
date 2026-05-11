/* global React, useViewerMode, useBreakpoint, useHashRoute, useCopy, useAnnounce, fmtRelative, shortSha */
// App shell — sidebar (desktop) / icon-rail (tablet) / topbar+drawer (mobile)
// + viewer-mode toggle, ref picker, breadcrumb host, copy-clone affordance.

const { useState, useEffect, useRef } = React;

const NAV = [
  { id: "publish",  label: "Publish",      glyph: "P", desc: "Repository overview & clone" },
  { id: "code",     label: "Code",         glyph: "C", desc: "Browse files & refs" },
  { id: "ai",       label: "AI objects",   glyph: "A", desc: "Intent · Plan · Run · Patch" },
];

// ────────────────────────────────────────────────────────────────────────────
function AppShell({ children }) {
  const route = useHashRoute();
  const bp = useBreakpoint();
  const { mode } = useViewerMode();
  const [drawerOpen, setDrawerOpen] = useState(false);

  // Close drawer on route change
  useEffect(() => { setDrawerOpen(false); }, [route.route, bp]);

  const repo = window.LIBRA_API.repository;

  return (
    <div style={{
      display: "flex",
      flexDirection: bp === "mobile" ? "column" : "row",
      minHeight: "100vh",
      background: "var(--paper)",
      color: "var(--ink)",
    }}>
      {bp === "mobile" && (
        <MobileTopbar repo={repo} onOpenDrawer={() => setDrawerOpen(true)} route={route} />
      )}
      {bp !== "mobile" && (
        <Sidebar bp={bp} mode={mode} repo={repo} route={route} />
      )}
      {bp === "mobile" && drawerOpen && (
        <MobileDrawer repo={repo} route={route} onClose={() => setDrawerOpen(false)} />
      )}
      <main style={{
        flex: 1,
        minWidth: 0,
        display: "flex",
        flexDirection: "column",
        background: "var(--paper)",
      }}>
        {children}
      </main>
    </div>
  );
}

// ── Desktop / tablet sidebar ────────────────────────────────────────────────
function Sidebar({ bp, mode, repo, route }) {
  const collapsed = bp === "tablet";
  const w = collapsed ? "var(--sidebar-w-tablet)" : "var(--sidebar-w-desktop)";

  return (
    <aside style={{
      width: w,
      flexShrink: 0,
      borderRight: "1px solid var(--paper-line)",
      background: "var(--paper-deep)",
      display: "flex",
      flexDirection: "column",
      position: "sticky",
      top: 0,
      height: "100vh",
    }}>
      <BrandBlock collapsed={collapsed} repo={repo} />
      <nav aria-label="Primary" style={{ padding: "8px 0", flex: 1, overflowY: "auto" }}>
        {NAV.map(n => (
          <NavItem key={n.id} item={n} active={route.route === n.id} collapsed={collapsed} />
        ))}
      </nav>
    </aside>
  );
}

function BrandBlock({ collapsed, repo }) {
  return (
    <div style={{
      padding: collapsed ? "16px 0" : "18px 18px 14px",
      borderBottom: "1px solid var(--paper-line)",
      display: "flex",
      flexDirection: "column",
      alignItems: collapsed ? "center" : "flex-start",
      gap: 6,
    }}>
      <div className="lb-brand" style={{ fontSize: collapsed ? 18 : 20 }}>
        Libra
      </div>
      {!collapsed && (
        <>
          <div className="lb-eyebrow" style={{ fontSize: 10, marginTop: 4 }}>Repository</div>
          <div className="lb-mono" style={{ fontSize: 12.5, color: "var(--ink-deep)", fontWeight: 600 }}>
            {repo.slug}
          </div>
          <div className="lb-meta" style={{ fontSize: 11.5 }}>
            {repo.description}
          </div>
        </>
      )}
    </div>
  );
}

function NavItem({ item, active, collapsed }) {
  const route = useHashRoute();
  const onClick = (e) => {
    e.preventDefault();
    route.navigate(item.id, [], {});
  };
  return (
    <a
      href={"#/" + item.id}
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: collapsed ? "12px 0" : "10px 18px",
        justifyContent: collapsed ? "center" : "flex-start",
        borderLeft: active ? "2px solid var(--gold)" : "2px solid transparent",
        background: active ? "var(--paper)" : "transparent",
        color: active ? "var(--ink-deep)" : "var(--ink-mid)",
        fontFamily: "var(--sans)",
        fontSize: 13.5,
        fontWeight: active ? 600 : 500,
        borderBottom: "0",
      }}
      title={collapsed ? item.label : undefined}
    >
      <span aria-hidden="true" style={{
        width: 22, height: 22,
        borderRadius: "var(--r-1)",
        background: active ? "var(--ink-deep)" : "var(--paper)",
        color: active ? "var(--paper)" : "var(--ink-mid)",
        border: "1px solid var(--paper-line)",
        display: "inline-flex", alignItems: "center", justifyContent: "center",
        fontFamily: "var(--mono)",
        fontSize: 11, fontWeight: 700,
      }}>{item.glyph}</span>
      {!collapsed && (
        <span style={{ flex: 1, minWidth: 0 }}>
          <span style={{ display: "block" }}>{item.label}</span>
          <span style={{
            display: "block",
            fontSize: 11,
            fontWeight: 400,
            color: "var(--ink-soft)",
            marginTop: 1,
          }}>{item.desc}</span>
        </span>
      )}
    </a>
  );
}

// (admin viewer mode removed — public-only build)

// ── Mobile chrome ───────────────────────────────────────────────────────────
function MobileTopbar({ repo, onOpenDrawer, route }) {
  return (
    <header style={{
      position: "sticky", top: 0, zIndex: 5,
      height: "var(--topbar-h)",
      background: "var(--paper-deep)",
      borderBottom: "1px solid var(--paper-line)",
      display: "flex",
      alignItems: "center",
      padding: "0 14px",
      gap: 12,
    }}>
      <button
        type="button"
        onClick={onOpenDrawer}
        aria-label="Open navigation"
        style={{
          width: 36, height: 36,
          borderRadius: "var(--r-1)",
          border: "1px solid var(--paper-line)",
          background: "var(--paper)",
          display: "inline-flex", alignItems: "center", justifyContent: "center",
        }}
      >
        <svg width="16" height="12" viewBox="0 0 16 12" aria-hidden="true">
          <rect y="0" width="16" height="1.6" fill="currentColor"/>
          <rect y="5" width="16" height="1.6" fill="currentColor"/>
          <rect y="10" width="16" height="1.6" fill="currentColor"/>
        </svg>
      </button>
      <div style={{ minWidth: 0, flex: 1 }}>
        <div className="lb-brand" style={{ fontSize: 16, lineHeight: 1 }}>Libra</div>
        <div className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)", marginTop: 2,
                                          overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {repo.slug} · {NAV.find(n => n.id === route.route)?.label || ""}
        </div>
      </div>
    </header>
  );
}

function MobileDrawer({ repo, route, onClose }) {
  const { mode } = useViewerMode();
  const ref = useRef(null);
  useEffect(() => {
    const onKey = (e) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    ref.current?.focus();
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Navigation"
      style={{
        position: "fixed", inset: 0, zIndex: 50,
        background: "rgba(14, 31, 54, 0.42)",
        display: "flex",
      }}
      onClick={onClose}
    >
      <div
        ref={ref}
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
        style={{
          width: "min(82vw, 320px)",
          background: "var(--paper)",
          borderRight: "1px solid var(--paper-line)",
          display: "flex",
          flexDirection: "column",
        }}
      >
        <div style={{
          padding: "14px 18px",
          borderBottom: "1px solid var(--paper-line)",
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          background: "var(--paper-deep)",
        }}>
          <div className="lb-brand" style={{ fontSize: 18 }}>Libra</div>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close navigation"
            style={{
              width: 32, height: 32,
              borderRadius: "var(--r-1)",
              border: "1px solid var(--paper-line)",
              background: "var(--paper)",
              fontFamily: "var(--mono)", fontSize: 12,
            }}
          >×</button>
        </div>
        <nav aria-label="Primary" style={{ padding: "8px 0", flex: 1, overflowY: "auto" }}>
          {NAV.map(n => (
            <NavItem key={n.id} item={n} active={route.route === n.id} collapsed={false} />
          ))}
        </nav>
      </div>
    </div>
  );
}

// ────────────────────────────────────────────────────────────────────────────
// PageHeader — section title bar with optional ref picker / breadcrumb / actions
// ────────────────────────────────────────────────────────────────────────────
function PageHeader({ eyebrow, title, subtitle, breadcrumb, refPicker, actions, sticky = true }) {
  return (
    <div style={{
      position: sticky ? "sticky" : "static",
      top: 0,
      zIndex: 4,
      background: "var(--paper)",
      borderBottom: "1px solid var(--paper-line)",
    }}>
      <div style={{
        padding: "18px clamp(16px, 3vw, 28px) 14px",
        display: "flex",
        flexDirection: "column",
        gap: 10,
      }}>
        {breadcrumb}
        <div style={{
          display: "flex",
          alignItems: "flex-end",
          justifyContent: "space-between",
          gap: 14,
          flexWrap: "wrap",
        }}>
          <div style={{ minWidth: 0, flex: "1 1 320px" }}>
            {eyebrow && <div className="lb-eyebrow" style={{ marginBottom: 4 }}>{eyebrow}</div>}
            {title && <h1 className="lb-h1">{title}</h1>}
            {subtitle && <div className="lb-meta" style={{ marginTop: 6 }}>{subtitle}</div>}
          </div>
          <div style={{ display: "flex", gap: 10, flexWrap: "wrap", alignItems: "center" }}>
            {refPicker}
            {actions}
          </div>
        </div>
      </div>
    </div>
  );
}

// ── Status pill, used everywhere ────────────────────────────────────────────
function StatusPill({ kind, children }) {
  const tones = {
    synced:    "good",
    syncing:   "info",
    stale:     "warn",
    error:     "bad",
    paused:    "warn",
    published: "good",
    publishing:"info",
    private:   "info",
    failed:    "bad",
    proposed:  "info",
    accepted:  "good",
    rejected:  "bad",
    superseded:"warn",
    draft:     "warn",
    succeeded: "good",
    running:   "info",
    pending:   "warn",
    completed: "good",
    passed:    "good",
  };
  const tone = tones[kind] || "info";
  return (
    <span className="lb-chip" data-tone={tone}>
      <span aria-hidden="true" style={{
        width: 6, height: 6, borderRadius: "50%",
        background: "currentColor", opacity: 0.7,
      }}/>
      {children || kind}
    </span>
  );
}

// ── Ref picker (branch / tag dropdown) ──────────────────────────────────────
function RefPicker({ refs, value, onChange }) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [tab, setTab] = useState("branch");
  const ref = useRef(null);
  useEffect(() => {
    if (!open) return;
    const onDoc = (e) => {
      if (ref.current && !ref.current.contains(e.target)) setOpen(false);
    };
    const onKey = (e) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", onDoc);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const current = refs.find(r => r.name === value) || refs[0];
  const filtered = refs.filter(r => r.type === tab && r.name.toLowerCase().includes(q.toLowerCase()));

  return (
    <div ref={ref} style={{ position: "relative" }}>
      <button
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen(o => !o)}
        style={{
          height: 34,
          padding: "0 12px",
          borderRadius: "var(--r-1)",
          border: "1px solid var(--paper-line)",
          background: "var(--paper)",
          display: "inline-flex",
          alignItems: "center",
          gap: 8,
          fontFamily: "var(--mono)",
          fontSize: 12.5,
          color: "var(--ink-deep)",
          minWidth: 180,
          maxWidth: 260,
        }}
      >
        <span aria-hidden="true" className="lb-eyebrow" style={{ fontSize: 10, color: "var(--ink-soft)" }}>
          {current.type === "tag" ? "tag" : "branch"}
        </span>
        <span style={{
          flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap",
          textAlign: "left",
        }}>{current.name}</span>
        <svg width="10" height="6" viewBox="0 0 10 6" aria-hidden="true">
          <path d="M0 0 L5 6 L10 0" fill="none" stroke="currentColor" strokeWidth="1.4"/>
        </svg>
      </button>
      {open && (
        <div role="listbox" aria-label="Refs" style={{
          position: "absolute",
          top: 38, left: 0,
          width: "min(360px, 90vw)",
          background: "var(--paper)",
          border: "1px solid var(--paper-line)",
          borderRadius: "var(--r-2)",
          boxShadow: "var(--shadow-2)",
          overflow: "hidden",
          zIndex: 10,
        }}>
          <div style={{ padding: 10, borderBottom: "1px solid var(--paper-line)" }}>
            <input
              autoFocus
              type="search"
              placeholder="Filter refs…"
              aria-label="Filter refs"
              value={q}
              onChange={e => setQ(e.target.value)}
              style={{
                width: "100%",
                padding: "6px 10px",
                border: "1px solid var(--paper-line)",
                borderRadius: "var(--r-1)",
                background: "var(--paper-deep)",
                fontFamily: "var(--mono)", fontSize: 12.5,
                color: "var(--ink-deep)",
              }}
            />
            <div role="tablist" aria-label="Ref type" style={{
              display: "grid", gridTemplateColumns: "1fr 1fr", marginTop: 8,
              border: "1px solid var(--paper-line)", borderRadius: "var(--r-1)", overflow: "hidden",
            }}>
              {["branch", "tag"].map(t => {
                const on = tab === t;
                const count = refs.filter(r => r.type === t).length;
                return (
                  <button
                    key={t}
                    role="tab" aria-selected={on}
                    onClick={() => setTab(t)}
                    style={{
                      padding: "6px 8px",
                      background: on ? "var(--ink-deep)" : "var(--paper)",
                      color: on ? "var(--paper)" : "var(--ink-mid)",
                      fontFamily: "var(--sans)", fontSize: 11.5, fontWeight: 600,
                      textAlign: "center",
                    }}
                  >{t === "branch" ? "Branches" : "Tags"} <span className="lb-mono" style={{ opacity: 0.7, marginLeft: 4 }}>{count}</span></button>
                );
              })}
            </div>
          </div>
          <div style={{ maxHeight: 280, overflowY: "auto" }}>
            {filtered.length === 0 && (
              <div className="lb-meta" style={{ padding: "16px", textAlign: "center" }}>No matches.</div>
            )}
            {filtered.map(r => {
              const sel = r.name === value;
              return (
                <button
                  key={r.name}
                  role="option"
                  aria-selected={sel}
                  onClick={() => { onChange(r.name); setOpen(false); }}
                  style={{
                    display: "grid",
                    gridTemplateColumns: "1fr auto",
                    rowGap: 2,
                    columnGap: 12,
                    width: "100%",
                    padding: "8px 12px",
                    borderBottom: "1px solid var(--paper-edge)",
                    background: sel ? "var(--paper-deep)" : "transparent",
                    borderLeft: sel ? "2px solid var(--gold)" : "2px solid transparent",
                  }}
                >
                  <span className="lb-mono" style={{ fontSize: 12.5, fontWeight: sel ? 600 : 500, color: "var(--ink-deep)",
                                                     overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {r.name}
                  </span>
                  <span style={{ display: "flex", gap: 6, alignItems: "center" }}>
                    {r.is_default && <span className="lb-chip" style={{ height: 18 }} data-tone="info">default</span>}
                    {r.protected && <span className="lb-chip" style={{ height: 18 }}>protected</span>}
                    <StatusPill kind={r.sync_state} />
                  </span>
                  <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>
                    {shortSha(r.oid)} · {fmtRelative(r.last_commit_at)} · {r.last_commit_author}
                    {r.type === "branch" && r.ahead != null && (r.ahead > 0 || r.behind > 0) && (
                      <> · <span style={{ color: "var(--good)" }}>↑{r.ahead}</span>
                          <span style={{ color: "var(--ink-faint)" }}> </span>
                          <span style={{ color: "var(--warn)" }}>↓{r.behind}</span></>
                    )}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

// ── Breadcrumb — clickable path segments ────────────────────────────────────
function Breadcrumb({ segments }) {
  // segments: [{label, onClick}]
  return (
    <nav aria-label="Breadcrumb" style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 4,
                                            fontFamily: "var(--mono)", fontSize: 12, color: "var(--ink-soft)" }}>
      {segments.map((s, i) => {
        const last = i === segments.length - 1;
        return (
          <React.Fragment key={i}>
            {s.onClick && !last ? (
              <button
                type="button"
                onClick={s.onClick}
                style={{
                  color: "var(--ink-mid)",
                  borderBottom: "1px solid transparent",
                  padding: "2px 0",
                }}
                onMouseEnter={(e) => e.currentTarget.style.borderBottomColor = "var(--paper-line)"}
                onMouseLeave={(e) => e.currentTarget.style.borderBottomColor = "transparent"}
              >{s.label}</button>
            ) : (
              <span style={{ color: last ? "var(--ink-deep)" : "var(--ink-soft)", fontWeight: last ? 600 : 400 }}>
                {s.label}
              </span>
            )}
            {!last && <span aria-hidden="true" style={{ color: "var(--ink-faint)" }}>/</span>}
          </React.Fragment>
        );
      })}
    </nav>
  );
}

// ── Copy button (icon + label) ──────────────────────────────────────────────
function CopyButton({ value, label = "Copy", copyKey }) {
  const { copy, copiedKey } = useCopy();
  const on = copiedKey === copyKey;
  return (
    <button
      type="button"
      onClick={() => copy(value, copyKey, "Command copied")}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        height: 28,
        padding: "0 10px",
        border: "1px solid var(--paper-line)",
        borderRadius: "var(--r-1)",
        background: on ? "var(--good-tint)" : "var(--paper)",
        color: on ? "var(--good)" : "var(--ink-mid)",
        fontFamily: "var(--sans)",
        fontSize: 11.5,
        fontWeight: 600,
        letterSpacing: "0.02em",
      }}
    >
      <svg width="11" height="11" viewBox="0 0 11 11" aria-hidden="true" fill="none">
        {on ? (
          <path d="M2 5.5 L4.5 8 L9 3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round"/>
        ) : (
          <>
            <rect x="3" y="3" width="6.5" height="6.5" rx="1" stroke="currentColor" strokeWidth="1"/>
            <path d="M1.5 7V2.5C1.5 1.95 1.95 1.5 2.5 1.5H7" stroke="currentColor" strokeWidth="1"/>
          </>
        )}
      </svg>
      {on ? "Copied" : label}
    </button>
  );
}

Object.assign(window, { AppShell, PageHeader, StatusPill, RefPicker, Breadcrumb, CopyButton, NAV });
