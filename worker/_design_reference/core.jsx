/* global React */
// Router · viewer-mode store · tiny utilities used across the app.
// Hash-based: #/route?ref=...&path=...&id=... — no build needed.

const { useState, useEffect, useCallback, useMemo, useRef, createContext, useContext } = React;

// ── Hash route parsing ──────────────────────────────────────────────────────
function parseHash(hash) {
  const raw = (hash || "#/").replace(/^#/, "");
  const [pathPart, queryPart] = raw.split("?");
  const segs = pathPart.split("/").filter(Boolean);
  const params = {};
  if (queryPart) {
    new URLSearchParams(queryPart).forEach((v, k) => { params[k] = v; });
  }
  return { route: segs[0] || "publish", segs, params };
}

function buildHash(route, segs = [], params = {}) {
  const path = "/" + [route, ...segs].filter(Boolean).join("/");
  const q = new URLSearchParams();
  Object.entries(params).forEach(([k, v]) => {
    if (v != null && v !== "") q.set(k, v);
  });
  const qs = q.toString();
  return "#" + path + (qs ? "?" + qs : "");
}

function useHashRoute() {
  const [state, setState] = useState(() => parseHash(window.location.hash));
  useEffect(() => {
    const onHash = () => setState(parseHash(window.location.hash));
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);
  const navigate = useCallback((route, segs, params) => {
    const next = buildHash(route, segs, params);
    if (next !== window.location.hash) window.location.hash = next;
  }, []);
  const updateParams = useCallback((patch) => {
    const cur = parseHash(window.location.hash);
    const merged = { ...cur.params };
    Object.entries(patch).forEach(([k, v]) => {
      if (v == null || v === "") delete merged[k];
      else merged[k] = v;
    });
    window.location.hash = buildHash(cur.route, cur.segs.slice(1), merged);
  }, []);
  return { ...state, navigate, updateParams };
}

// ── Viewport breakpoint ─────────────────────────────────────────────────────
function useBreakpoint() {
  const get = () => {
    const w = window.innerWidth;
    if (w < 720) return "mobile";
    if (w < 1080) return "tablet";
    return "desktop";
  };
  const [bp, setBp] = useState(get);
  useEffect(() => {
    const onR = () => setBp(get());
    window.addEventListener("resize", onR);
    return () => window.removeEventListener("resize", onR);
  }, []);
  return bp;
}

// ── Viewer mode (public | admin) ────────────────────────────────────────────
const ViewerModeContext = createContext({ mode: "public", setMode: () => {} });

function ViewerModeProvider({ children }) {
  // Public-only: admin viewer mode has been removed.
  return (
    <ViewerModeContext.Provider value={{ mode: "public", setMode: () => {} }}>
      {children}
    </ViewerModeContext.Provider>
  );
}

const useViewerMode = () => useContext(ViewerModeContext);

// ── Live region (announcements) ────────────────────────────────────────────
const LiveContext = createContext({ announce: () => {} });

function LiveRegionProvider({ children }) {
  const [msg, setMsg] = useState("");
  const announce = useCallback((m) => {
    setMsg("");
    requestAnimationFrame(() => setMsg(m));
  }, []);
  return (
    <LiveContext.Provider value={{ announce }}>
      {children}
      <div role="status" aria-live="polite" className="lb-sr">{msg}</div>
    </LiveContext.Provider>
  );
}

const useAnnounce = () => useContext(LiveContext).announce;

// ── Copy-to-clipboard with announcement ────────────────────────────────────
function useCopy() {
  const announce = useAnnounce();
  const [copiedKey, setCopiedKey] = useState(null);
  const timeoutRef = useRef();
  const copy = useCallback(async (text, key = "default", label = "Copied") => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedKey(key);
      announce(label);
      clearTimeout(timeoutRef.current);
      timeoutRef.current = setTimeout(() => setCopiedKey(null), 1600);
    } catch {
      announce("Copy failed");
    }
  }, [announce]);
  return { copy, copiedKey };
}

// ── Formatters ──────────────────────────────────────────────────────────────
function fmtBytes(n) {
  if (n == null) return "—";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} kB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
function fmtDateTime(s) {
  if (!s) return "—";
  const d = new Date(s);
  return d.toISOString().slice(0, 16).replace("T", " ") + "Z";
}
function fmtDate(s) {
  if (!s) return "—";
  const d = new Date(s);
  return d.toISOString().slice(0, 10);
}
function fmtRelative(s) {
  if (!s) return "—";
  const ms = Date.now() - new Date(s).getTime();
  const m = Math.round(ms / 60000);
  if (m < 1)   return "just now";
  if (m < 60)  return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24)  return `${h}h ago`;
  const d = Math.round(h / 24);
  if (d < 30)  return `${d}d ago`;
  const mo = Math.round(d / 30);
  return `${mo}mo ago`;
}
function shortSha(s) {
  return s ? String(s).slice(0, 8) : "—";
}

Object.assign(window, {
  parseHash, buildHash, useHashRoute,
  useBreakpoint,
  ViewerModeProvider, useViewerMode,
  LiveRegionProvider, useAnnounce,
  useCopy,
  fmtBytes, fmtDateTime, fmtDate, fmtRelative, shortSha,
});
