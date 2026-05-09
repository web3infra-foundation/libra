/* global React, ScreenWithChrome, StatusPill, fmtBytes */
// Code Browser — repo tree on the left, file list on the right, header summary above.

function CodeBrowser() {
  const { repository, file_tree } = window.LIBRA_API;

  // Group tree into a simple two-pane layout: directories on the left,
  // contents of `src/ledger` on the right (the focused folder).
  const focusedFolder = "src/ledger";
  const focusedFiles = file_tree.filter(n =>
    n.path.startsWith(focusedFolder + "/") &&
    n.path.split("/").length === focusedFolder.split("/").length + 1
  );

  // Top-level folders for the tree pane
  const topNodes = file_tree.filter(n =>
    !n.path.includes("/") || (n.path.split("/").length === 2 && n.path.startsWith("src/"))
  );

  return (
    <ScreenWithChrome
      active="browse"
      repo={repository}
      crumbs={["kepler-ledger", "main", "src", "ledger"]}
      actions={
        <>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>
            head {repository.head_sha}
          </span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>·</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>
            {repository.file_count.toLocaleString()} files
          </span>
        </>
      }
    >
      {/* Two-column body */}
      <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
        {/* TREE */}
        <div style={{
          width: 280,
          borderRight: "1px solid var(--paper-line)",
          padding: "18px 0",
          overflow: "auto",
        }}>
          <div className="lb-eyebrow" style={{ padding: "0 22px 10px" }}>Tree</div>
          <TreeNode label="kepler-ledger" depth={0} expanded root />
            <TreeNode label="src" depth={1} expanded folder count={42} />
              <TreeNode label="ledger" depth={2} expanded folder count={18} active />
              <TreeNode label="migrations" depth={2} folder count={14} />
              <TreeNode label="index.ts" depth={2} />
            <TreeNode label="vendor" depth={1} folder count={312} ignored />
            <TreeNode label="dist" depth={1} folder count={87} ignored />
            <TreeNode label="logs" depth={1} folder count={9} />
            <TreeNode label="README.md" depth={1} />
            <TreeNode label="package.json" depth={1} />
            <TreeNode label=".gitignore" depth={1} muted />
        </div>

        {/* FILE LIST */}
        <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column" }}>
          <FolderHeader folder={focusedFolder} count={focusedFiles.length} />

          <div style={{
            display: "grid",
            gridTemplateColumns: "minmax(0, 1fr) 110px 100px 180px",
            padding: "10px 28px",
            borderBottom: "1px solid var(--paper-line)",
            fontFamily: "var(--sans)",
            fontSize: 10.5,
            letterSpacing: "0.12em",
            textTransform: "uppercase",
            color: "var(--ink-soft)",
          }}>
            <span>Name</span>
            <span style={{ textAlign: "right" }}>Size</span>
            <span style={{ textAlign: "right" }}>Lines</span>
            <span>Last change</span>
          </div>

          <div style={{ overflow: "auto", flex: 1 }}>
            {focusedFiles.map((f, i) => <FileRow key={f.path} f={f} idx={i} />)}
          </div>
        </div>
      </div>
    </ScreenWithChrome>
  );
}

function FolderHeader({ folder, count }) {
  return (
    <div style={{
      padding: "22px 28px 16px",
      borderBottom: "1px solid var(--paper-line)",
      display: "flex",
      alignItems: "flex-end",
      justifyContent: "space-between",
      gap: 24,
    }}>
      <div>
        <div className="lb-eyebrow" style={{ marginBottom: 6 }}>Folder</div>
        <h1 className="lb-h1" style={{ fontFamily: "var(--mono)", fontSize: 22, fontWeight: 500 }}>
          {folder}
        </h1>
        <div className="lb-meta" style={{ marginTop: 6 }}>
          {count} entries · core ledger primitives · last touched May 06
        </div>
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <span className="lb-chip">TypeScript</span>
        <span className="lb-chip dot" style={{ color: "var(--gold)", borderColor: "var(--gold)" }}>
          1 redacted
        </span>
      </div>
    </div>
  );
}

function TreeNode({ label, depth, expanded, folder, root, count, active, ignored, muted }) {
  const indent = 22 + depth * 16;
  return (
    <div style={{
      display: "flex",
      alignItems: "center",
      gap: 8,
      padding: "5px 22px 5px 0",
      paddingLeft: indent,
      background: active ? "var(--paper-deep)" : "transparent",
      borderLeft: active ? "2px solid var(--gold)" : "2px solid transparent",
      fontFamily: folder || root ? "var(--serif)" : "var(--mono)",
      fontSize: folder || root ? 13.5 : 12,
      color: ignored ? "var(--ink-faint)" : muted ? "var(--ink-soft)" : "var(--ink)",
      fontWeight: active ? 600 : (root ? 600 : 400),
      fontStyle: ignored ? "italic" : "normal",
    }}>
      <span style={{
        width: 10, color: "var(--ink-soft)",
        fontFamily: "var(--mono)", fontSize: 9,
        opacity: folder || root ? 1 : 0,
      }}>
        {expanded ? "▾" : "▸"}
      </span>
      <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
        {label}
      </span>
      {ignored && (
        <span style={{ fontFamily: "var(--sans)", fontSize: 10, color: "var(--ink-faint)" }}>ignored</span>
      )}
      {count != null && !ignored && (
        <span style={{ fontFamily: "var(--mono)", fontSize: 10, color: "var(--ink-soft)" }}>{count}</span>
      )}
    </div>
  );
}

function FileRow({ f, idx }) {
  const name = f.path.split("/").pop();
  const lines = f.is_binary ? "—" : f.is_too_large ? "—" : Math.round((f.size_bytes || 0) / 32);
  const flagChips = [];
  if (f.is_binary)       flagChips.push({ label: "Binary",     k: "neutral" });
  if (f.is_too_large)    flagChips.push({ label: "Too large",  k: "warn" });
  if (f.is_ignored)      flagChips.push({ label: "Ignored",    k: "muted" });
  if (f.has_redactions)  flagChips.push({ label: "Redacted",   k: "gold" });

  return (
    <div style={{
      display: "grid",
      gridTemplateColumns: "minmax(0, 1fr) 110px 100px 180px",
      padding: "12px 28px",
      borderBottom: "1px solid var(--paper-edge)",
      alignItems: "center",
      background: idx === 0 ? "var(--paper-deep)" : "transparent",
    }}>
      <div style={{ display: "flex", alignItems: "center", gap: 12, minWidth: 0 }}>
        <FileGlyph language={f.language} binary={f.is_binary} />
        <div style={{ minWidth: 0 }}>
          <div style={{
            fontFamily: "var(--mono)",
            fontSize: 12.5,
            color: "var(--ink)",
            fontWeight: 500,
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}>{name}</div>
          <div style={{ display: "flex", gap: 6, marginTop: 4, flexWrap: "wrap" }}>
            {flagChips.map((c, i) => <FlagChip key={i} {...c} />)}
            {flagChips.length === 0 && (
              <span style={{ fontFamily: "var(--sans)", fontSize: 10.5, color: "var(--ink-soft)" }}>
                {f.language || "—"}
              </span>
            )}
          </div>
        </div>
      </div>
      <div className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink-mid)", textAlign: "right" }}>
        {fmtBytes(f.size_bytes || 0)}
      </div>
      <div className="lb-mono" style={{ fontSize: 11.5, color: "var(--ink-mid)", textAlign: "right" }}>
        {lines}
      </div>
      <div className="lb-meta">May 0{(idx % 6) + 1} · m.ostrowski</div>
    </div>
  );
}

function FlagChip({ label, k }) {
  const styles = {
    neutral: { color: "var(--ink-mid)", border: "var(--paper-line)", bg: "transparent" },
    warn:    { color: "var(--warn)",    border: "var(--warn)",       bg: "transparent" },
    muted:   { color: "var(--ink-soft)", border: "var(--paper-line)", bg: "transparent" },
    gold:    { color: "var(--gold)",    border: "var(--gold)",       bg: "transparent" },
  }[k];
  return (
    <span style={{
      fontFamily: "var(--sans)",
      fontSize: 9.5,
      letterSpacing: "0.1em",
      textTransform: "uppercase",
      padding: "1px 6px",
      borderRadius: 2,
      border: `1px solid ${styles.border}`,
      color: styles.color,
      background: styles.bg,
    }}>{label}</span>
  );
}

function FileGlyph({ language, binary }) {
  const ext = {
    typescript: "TS",
    javascript: "JS",
    json: "{ }",
    markdown: "MD",
    sql: "SQL",
    log: "LOG",
    dotenv: "ENV",
    gitignore: "GIT",
  }[language] || (binary ? "BIN" : "···");
  return (
    <div style={{
      width: 30, height: 36,
      borderRadius: "var(--r-1)",
      border: "1px solid var(--paper-line)",
      background: "var(--paper)",
      display: "flex",
      alignItems: "flex-end",
      justifyContent: "center",
      paddingBottom: 4,
      fontFamily: "var(--mono)",
      fontSize: 8.5,
      color: "var(--ink-mid)",
      letterSpacing: "0.04em",
      flexShrink: 0,
      position: "relative",
    }}>
      <span style={{
        position: "absolute",
        top: 4, left: 4, right: 4,
        height: 1, background: "var(--gold)", opacity: 0.5,
      }} />
      {ext}
    </div>
  );
}

window.CodeBrowser = CodeBrowser;
