/* global React, ScreenWithChrome */
// Empty + Error states collection — laid out as a single screen-sized board
// of stacked cards. Treats errors as editorial entries, not danger.

const REPO4 = () => window.LIBRA_API.repository;
const ERRS = () => window.LIBRA_API.error_examples;

function EmptyAndErrorStates() {
  return (
    <ScreenWithChrome
      active="audit"
      repo={REPO4()}
      crumbs={["kepler-ledger", "States", "Empty & error"]}
      actions={<span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>9 surfaces · 6 error_object codes</span>}
    >
      <div style={{ padding: "22px 28px 18px", borderBottom: "1px solid var(--paper-line)" }}>
        <div className="lb-eyebrow" style={{ marginBottom: 6 }}>Reference sheet</div>
        <h1 className="lb-h1">Empty & error states</h1>
        <div className="lb-meta" style={{ marginTop: 6 }}>
          Six error_object codes from §6 plus three empty-state surfaces. Tone is matter-of-fact, never alarmed.
        </div>
      </div>

      <div style={{ overflow: "auto", flex: 1, padding: 28, background: "var(--paper-deep)" }}>
        {/* Empty states */}
        <div className="lb-eyebrow" style={{ marginBottom: 14 }}>Empty</div>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 18, marginBottom: 28 }}>
          <EmptyCard
            title="No AI versions yet."
            body="Versions appear here once one is generated or scheduled for this file."
            field={["ai_versions", "[]"]}
          />
          <EmptyCard
            title="Folder is empty."
            body="No tracked files at this path. Files matched by .libraignore are hidden."
            field={["file_tree", "[]"]}
          />
          <EmptyCard
            title="No sync events in window."
            body="No webhooks or scheduled jobs ran in the last 24 hours."
            field={["sync_events", "[] · since=24h"]}
          />
        </div>

        {/* Errors */}
        <div className="lb-eyebrow" style={{ marginBottom: 14 }}>Errors · error_object</div>
        <div style={{
          border: "1px solid var(--paper-line)",
          borderRadius: "var(--r-2)",
          background: "var(--paper)",
        }}>
          <div style={{
            display: "grid",
            gridTemplateColumns: "200px 60px 1fr 200px",
            padding: "10px 18px",
            borderBottom: "1px solid var(--paper-line)",
            fontFamily: "var(--sans)",
            fontSize: 10.5,
            letterSpacing: "0.12em",
            textTransform: "uppercase",
            color: "var(--ink-soft)",
          }}>
            <span>code</span>
            <span>http</span>
            <span>message</span>
            <span>recovery</span>
          </div>
          {ERRS().map((e, i) => <ErrorRow key={e.code} e={e} idx={i} last={i === ERRS().length - 1} />)}
        </div>
      </div>
    </ScreenWithChrome>
  );
}

function EmptyCard({ title, body, field }) {
  return (
    <div style={{
      border: "1px solid var(--paper-line)",
      borderTop: "2px solid var(--ink-faint)",
      borderRadius: "var(--r-2)",
      background: "var(--paper)",
      padding: 22,
      display: "flex",
      flexDirection: "column",
      gap: 12,
      minHeight: 200,
    }}>
      <div style={{
        fontFamily: "var(--serif)",
        fontSize: 17,
        fontWeight: 500,
        color: "var(--ink)",
        letterSpacing: "-0.005em",
      }}>{title}</div>
      <div style={{
        fontFamily: "var(--serif)",
        fontSize: 14,
        color: "var(--ink-mid)",
        lineHeight: 1.5,
        textWrap: "pretty",
        flex: 1,
      }}>{body}</div>
      <div style={{
        borderTop: "1px solid var(--paper-edge)",
        paddingTop: 12,
      }}>
        <span className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)" }}>
          {field[0]} = {field[1]}
        </span>
      </div>
    </div>
  );
}

function ErrorRow({ e, idx, last }) {
  const recoveryMap = {
    FILE_NOT_FOUND:      "Pick a different blob_sha or branch",
    BLOB_TOO_LARGE:      "Stream first 200 lines · download raw",
    BINARY_NOT_VIEWABLE: "Open hex preview · download raw",
    REPO_NOT_INDEXED:    "Wait for index_completed event",
    RATE_LIMITED:        "Auto-retry in 90s",
    INTEGRATION_REVOKED: "Reinstall GitHub App",
  };
  return (
    <div style={{
      display: "grid",
      gridTemplateColumns: "200px 60px 1fr 200px",
      padding: "14px 18px",
      borderBottom: last ? "none" : "1px solid var(--paper-edge)",
      alignItems: "flex-start",
      gap: 16,
      background: idx === 0 ? "var(--paper-deep)" : "transparent",
    }}>
      <div className="lb-mono" style={{ fontSize: 12, color: "var(--ink)", fontWeight: 600 }}>
        {e.code}
      </div>
      <div className="lb-mono" style={{ fontSize: 12, color: "var(--ink-mid)" }}>
        {e.http}
      </div>
      <div style={{
        fontFamily: "var(--serif)",
        fontSize: 14,
        color: "var(--ink-mid)",
        lineHeight: 1.45,
        textWrap: "pretty",
      }}>{e.message}</div>
      <div style={{
        fontFamily: "var(--sans)",
        fontSize: 11.5,
        color: "var(--ink-mid)",
      }}>{recoveryMap[e.code]}</div>
    </div>
  );
}

window.EmptyAndErrorStates = EmptyAndErrorStates;
