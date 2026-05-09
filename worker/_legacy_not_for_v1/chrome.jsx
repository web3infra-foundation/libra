/* global React */
// Shared chrome — sidebar nav, header, footer status bar, generic layout shell.
// Uses tokens from tokens.css. Inline styles only for layout; visual styling stays in CSS.

const { Fragment } = React;

// ─────────────────────────────────────────────────────────────────────────
// Wordmark
// ─────────────────────────────────────────────────────────────────────────
function LibraWordmark({ size = 22 }) {
  return (
    <div style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
      <span style={{
        fontFamily: "var(--serif)",
        fontWeight: 500,
        fontSize: size,
        letterSpacing: "-0.01em",
        color: "var(--ink)",
      }}>Libra</span>
      <span style={{
        fontFamily: "var(--mono)",
        fontSize: 10,
        color: "var(--ink-soft)",
        letterSpacing: "0.08em",
      }}>v0.4 · 2026</span>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Sidebar — repo picker + nav
// ─────────────────────────────────────────────────────────────────────────
function Sidebar({ active = "browse", repo }) {
  const items = [
    { id: "browse",   label: "Browse",        sub: "Files & code" },
    { id: "versions", label: "AI Versions",   sub: "Drafts · review" },
    { id: "sync",     label: "Sync",          sub: "Index & webhooks" },
    { id: "audit",    label: "Audit Log",     sub: "Read-only history" },
    { id: "settings", label: "Settings",      sub: "Redaction · access" },
  ];
  return (
    <aside style={{
      width: 232,
      flexShrink: 0,
      borderRight: "1px solid var(--paper-line)",
      background: "var(--paper)",
      padding: "20px 0",
      display: "flex",
      flexDirection: "column",
    }}>
      <div style={{ padding: "0 22px 18px" }}>
        <LibraWordmark />
      </div>

      <hr className="lb-rule" style={{ margin: "0 22px 16px" }} />

      <div style={{ padding: "0 22px 14px" }}>
        <div className="lb-eyebrow" style={{ marginBottom: 6 }}>Repository</div>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
          <div>
            <div style={{ fontFamily: "var(--serif)", fontSize: 15, color: "var(--ink)", lineHeight: 1.2 }}>
              {repo.name}
            </div>
            <div style={{ fontFamily: "var(--mono)", fontSize: 10.5, color: "var(--ink-soft)", marginTop: 3 }}>
              {repo.default_branch} · {repo.head_sha}
            </div>
          </div>
          <span className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)" }}>▾</span>
        </div>
      </div>

      <hr className="lb-rule" style={{ margin: "4px 22px 14px" }} />

      <nav style={{ display: "flex", flexDirection: "column" }}>
        {items.map(it => {
          const on = it.id === active;
          return (
            <div key={it.id} style={{
              padding: "9px 22px",
              borderLeft: on ? "2px solid var(--gold)" : "2px solid transparent",
              background: on ? "var(--paper-deep)" : "transparent",
            }}>
              <div style={{
                fontFamily: "var(--sans)",
                fontSize: 13,
                fontWeight: on ? 600 : 500,
                color: on ? "var(--ink)" : "var(--ink-mid)",
                letterSpacing: "0.01em",
              }}>{it.label}</div>
              <div style={{
                fontFamily: "var(--sans)",
                fontSize: 10.5,
                color: "var(--ink-soft)",
                marginTop: 1,
              }}>{it.sub}</div>
            </div>
          );
        })}
      </nav>

      <div style={{ flex: 1 }} />

      <div style={{ padding: "14px 22px", borderTop: "1px solid var(--paper-line)" }}>
        <div className="lb-eyebrow" style={{ marginBottom: 6 }}>Workspace</div>
        <div style={{ fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink)" }}>Ostrowski Co.</div>
        <div style={{ fontFamily: "var(--mono)", fontSize: 10.5, color: "var(--ink-soft)" }}>3 members · 12 repos</div>
      </div>
    </aside>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Top header — breadcrumb + actions
// ─────────────────────────────────────────────────────────────────────────
function TopBar({ crumbs = [], actions = null }) {
  return (
    <div style={{
      height: 56,
      borderBottom: "1px solid var(--paper-line)",
      display: "flex",
      alignItems: "center",
      padding: "0 28px",
      gap: 14,
      background: "var(--paper)",
    }}>
      <div style={{
        display: "flex",
        alignItems: "baseline",
        gap: 8,
        fontFamily: "var(--mono)",
        fontSize: 12,
        color: "var(--ink-mid)",
        flex: 1,
        minWidth: 0,
      }}>
        {crumbs.map((c, i) => (
          <span key={i} style={{ display: "contents" }}>
            {i > 0 && <span style={{ color: "var(--ink-faint)" }}>/</span>}
            <span style={{
              color: i === crumbs.length - 1 ? "var(--ink)" : "var(--ink-mid)",
              fontWeight: i === crumbs.length - 1 ? 600 : 400,
            }}>{c}</span>
          </span>
        ))}
      </div>
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>{actions}</div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Sync indicator (compact, used in headers)
// ─────────────────────────────────────────────────────────────────────────
function SyncDot({ state }) {
  const map = {
    synced:  { color: "var(--good)", label: "Synced" },
    syncing: { color: "var(--gold)", label: "Indexing" },
    stale:   { color: "var(--warn)", label: "Stale" },
    error:   { color: "var(--ink)", label: "Sync error" },
    paused:  { color: "var(--quiet)", label: "Paused" },
  };
  const s = map[state] || map.synced;
  return (
    <span style={{
      display: "inline-flex",
      alignItems: "center",
      gap: 6,
      fontFamily: "var(--sans)",
      fontSize: 11.5,
      color: "var(--ink-mid)",
    }}>
      <span style={{
        width: 7, height: 7, borderRadius: "50%",
        background: s.color,
        boxShadow: state === "syncing" ? `0 0 0 2px ${"rgba(183,135,58,0.18)"}` : "none",
      }} />
      <span style={{ letterSpacing: "0.02em" }}>{s.label}</span>
    </span>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Footer / status strip
// ─────────────────────────────────────────────────────────────────────────
function StatusBar({ repo }) {
  return (
    <div style={{
      height: 28,
      borderTop: "1px solid var(--paper-line)",
      background: "var(--paper-deep)",
      display: "flex",
      alignItems: "center",
      padding: "0 18px",
      gap: 16,
      fontFamily: "var(--mono)",
      fontSize: 10.5,
      color: "var(--ink-soft)",
      letterSpacing: "0.02em",
    }}>
      <SyncDot state={repo.sync_state} />
      <span>·</span>
      <span>head {repo.head_sha}</span>
      <span>·</span>
      <span>{repo.file_count.toLocaleString()} files</span>
      <span>·</span>
      <span>{(repo.storage_bytes / (1024 * 1024)).toFixed(1)} MB indexed</span>
      <div style={{ flex: 1 }} />
      <span>last index 09:14:22 UTC</span>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Layout shell — frames a screen at fixed dimensions for the canvas
// ─────────────────────────────────────────────────────────────────────────
function Screen({ width = 1280, height = 820, children }) {
  return (
    <div className="lb-app" style={{
      width, height,
      background: "var(--paper)",
      display: "flex",
      flexDirection: "column",
      overflow: "hidden",
      position: "relative",
    }}>{children}</div>
  );
}

function ScreenWithChrome({ active, repo, crumbs, actions, children, width = 1280, height = 820 }) {
  return (
    <Screen width={width} height={height}>
      <div style={{ display: "flex", flex: 1, minHeight: 0 }}>
        <Sidebar active={active} repo={repo} />
        <div style={{ display: "flex", flexDirection: "column", flex: 1, minWidth: 0 }}>
          <TopBar crumbs={crumbs} actions={actions} />
          <div style={{ flex: 1, minHeight: 0, overflow: "hidden", display: "flex", flexDirection: "column" }}>
            {children}
          </div>
        </div>
      </div>
      <StatusBar repo={repo} />
    </Screen>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Status pill — versions, sync events, errors
// ─────────────────────────────────────────────────────────────────────────
function StatusPill({ kind }) {
  const map = {
    proposed:    { label: "Proposed",    fg: "var(--gold)",   bg: "transparent", bd: "var(--gold)" },
    accepted:    { label: "Accepted",    fg: "var(--paper)",  bg: "var(--ink)",  bd: "var(--ink)" },
    rejected:    { label: "Rejected",    fg: "var(--ink-mid)", bg: "transparent", bd: "var(--paper-line)" },
    superseded:  { label: "Superseded",  fg: "var(--ink-soft)", bg: "transparent", bd: "var(--paper-line)" },
    draft:       { label: "Draft",       fg: "var(--ink-mid)", bg: "var(--paper-deep)", bd: "var(--paper-line)" },
    info:        { label: "Info",        fg: "var(--info)",   bg: "transparent", bd: "var(--paper-line)" },
    warn:        { label: "Warn",        fg: "var(--warn)",   bg: "transparent", bd: "var(--warn)" },
    error:       { label: "Error",       fg: "var(--ink)",    bg: "transparent", bd: "var(--ink)" },
  };
  const s = map[kind] || map.info;
  return (
    <span style={{
      display: "inline-flex",
      alignItems: "center",
      padding: "2px 8px",
      borderRadius: "var(--r-1)",
      border: `1px solid ${s.bd}`,
      background: s.bg,
      color: s.fg,
      fontFamily: "var(--sans)",
      fontSize: 10.5,
      letterSpacing: "0.06em",
      textTransform: "uppercase",
      fontWeight: 600,
    }}>{s.label}</span>
  );
}

// Time helpers
function fmtDateTime(iso) {
  const d = new Date(iso);
  const date = d.toLocaleDateString("en-US", { month: "short", day: "2-digit", year: "numeric" });
  const time = d.toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit", hour12: false });
  return `${date} · ${time}`;
}
function fmtBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n/1024).toFixed(1)} KB`;
  return `${(n/1024/1024).toFixed(1)} MB`;
}

Object.assign(window, {
  LibraWordmark, Sidebar, TopBar, SyncDot, StatusBar,
  Screen, ScreenWithChrome, StatusPill,
  fmtDateTime, fmtBytes,
});
