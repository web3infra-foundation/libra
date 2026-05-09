/* global React, ScreenWithChrome, StatusPill, fmtDateTime, SyncDot */
// Sync Status — health overview + event log

const REPO3 = () => window.LIBRA_API.repository;
const EVENTS = () => window.LIBRA_API.sync_events;

function SyncStatus() {
  const repo = REPO3();
  const events = EVENTS();
  return (
    <ScreenWithChrome
      active="sync"
      repo={repo}
      crumbs={["kepler-ledger", "Sync"]}
      actions={
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>last index 09:14:22 UTC</span>
      }
    >
      {/* Header summary */}
      <div style={{ padding: "22px 28px 18px", borderBottom: "1px solid var(--paper-line)" }}>
        <div className="lb-eyebrow" style={{ marginBottom: 6 }}>Sync</div>
        <h1 className="lb-h1">Repository is up to date.</h1>
        <div className="lb-meta" style={{ marginTop: 6 }}>
          GitHub App · webhook listener · last index 8.3 s ago.
        </div>
      </div>

      {/* Health cards row */}
      <div style={{
        display: "grid",
        gridTemplateColumns: "repeat(4, 1fr)",
        gap: 0,
        borderBottom: "1px solid var(--paper-line)",
      }}>
        <HealthCard label="Index" value="Synced" detail="1,247 files · 184.3 MB" tone="ink" leading />
        <HealthCard label="Webhooks" value="Listening" detail="last received 09:14:11 UTC" tone="ink" />
        <HealthCard label="Rate limit" value="4,872 / 5,000" detail="resets in 23 min" tone="warn" />
        <HealthCard label="Auth" value="Healthy" detail="token rotates 06:00 UTC daily" tone="ink" trailing />
      </div>

      {/* Body — two columns */}
      <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
        {/* Event log */}
        <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column" }}>
          <div style={{
            padding: "16px 28px 10px",
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            borderBottom: "1px solid var(--paper-line)",
          }}>
            <div className="lb-eyebrow">Event log · last 24h</div>
            <div className="lb-meta lb-mono" style={{ fontSize: 11 }}>showing 6 of 412</div>
          </div>
          <div style={{ overflow: "auto", flex: 1 }}>
            {events.map((e, i) => <EventRow key={e.event_id} e={e} idx={i} />)}
          </div>
        </div>

        {/* Right rail — schedule + integration */}
        <aside style={{
          width: 320,
          borderLeft: "1px solid var(--paper-line)",
          background: "var(--paper-deep)",
          padding: "18px 22px",
          overflow: "auto",
        }}>
          <div className="lb-eyebrow" style={{ marginBottom: 12 }}>Schedule</div>
          <div style={{
            border: "1px solid var(--paper-line)",
            borderRadius: "var(--r-2)",
            background: "var(--paper)",
            padding: 14,
            marginBottom: 18,
          }}>
            <ScheduleRow time="every push" label="Incremental index" via="webhook" />
            <ScheduleRow time="06:00 UTC" label="Full reindex" via="cron" />
            <ScheduleRow time="hourly" label="Embedding refresh" via="cron" last />
          </div>

          <div className="lb-eyebrow" style={{ marginBottom: 12 }}>Integration</div>
          <div style={{
            border: "1px solid var(--paper-line)",
            borderRadius: "var(--r-2)",
            background: "var(--paper)",
            padding: 14,
            marginBottom: 18,
          }}>
            <IntegrationKV k="provider" v="GitHub App" />
            <IntegrationKV k="installation_id" v="ins_4729" />
            <IntegrationKV k="webhook_url" v="…/hooks/rp_8f4c1b" />
            <IntegrationKV k="signing" v="HMAC-SHA256" />
            <IntegrationKV k="scope" v="contents.read · meta.read" last />
          </div>

          <div className="lb-eyebrow" style={{ marginBottom: 12 }}>Storage</div>
          <div style={{
            border: "1px solid var(--paper-line)",
            borderRadius: "var(--r-2)",
            background: "var(--paper)",
            padding: 14,
          }}>
            <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 8 }}>
              <span className="lb-meta">Used</span>
              <span className="lb-mono" style={{ fontSize: 12, color: "var(--ink)" }}>184.3 MB / 2.0 GB</span>
            </div>
            <div style={{
              height: 6,
              borderRadius: 3,
              background: "var(--paper-deep)",
              overflow: "hidden",
              border: "1px solid var(--paper-line)",
            }}>
              <div style={{ width: "9.2%", height: "100%", background: "var(--ink)" }} />
            </div>
            <div className="lb-meta" style={{ marginTop: 8, fontSize: 11 }}>
              Embeddings 76.4 MB · raw blobs 102.1 MB · index 5.8 MB
            </div>
          </div>
        </aside>
      </div>
    </ScreenWithChrome>
  );
}

function HealthCard({ label, value, detail, tone, leading, trailing }) {
  const accent = tone === "warn" ? "var(--gold)" : "var(--ink)";
  return (
    <div style={{
      padding: "20px 24px",
      borderRight: trailing ? "none" : "1px solid var(--paper-line)",
      borderTop: `2px solid ${accent}`,
      background: "var(--paper)",
    }}>
      <div className="lb-eyebrow" style={{ marginBottom: 8, color: accent }}>{label}</div>
      <div style={{
        fontFamily: "var(--serif)",
        fontSize: 22,
        fontWeight: 500,
        color: "var(--ink)",
        lineHeight: 1.1,
        letterSpacing: "-0.005em",
      }}>{value}</div>
      <div className="lb-meta" style={{ marginTop: 6 }}>{detail}</div>
    </div>
  );
}

function EventRow({ e, idx }) {
  const levelMap = {
    info:  { color: "var(--info)", label: "Info" },
    warn:  { color: "var(--warn)", label: "Warn" },
    error: { color: "var(--ink)", label: "Error" },
  };
  const l = levelMap[e.level];

  return (
    <div style={{
      display: "grid",
      gridTemplateColumns: "70px 200px 1fr 120px",
      padding: "14px 28px",
      borderBottom: "1px solid var(--paper-edge)",
      alignItems: "flex-start",
      gap: 16,
      background: idx === 0 ? "var(--paper-deep)" : "transparent",
    }}>
      <div style={{
        fontFamily: "var(--sans)",
        fontSize: 10,
        letterSpacing: "0.08em",
        textTransform: "uppercase",
        color: l.color,
        fontWeight: 600,
        paddingTop: 2,
      }}>
        {l.label}
      </div>
      <div>
        <div className="lb-mono" style={{ fontSize: 12, color: "var(--ink)", fontWeight: 500 }}>
          {e.event_type}
        </div>
        <div className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink-soft)", marginTop: 2 }}>
          {e.event_id}
        </div>
      </div>
      <div style={{
        fontFamily: "var(--serif)",
        fontSize: 14,
        color: "var(--ink-mid)",
        lineHeight: 1.5,
        textWrap: "pretty",
      }}>{e.detail}</div>
      <div className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)", textAlign: "right" }}>
        {fmtDateTime(e.at)}
      </div>
    </div>
  );
}

function ScheduleRow({ time, label, via, last }) {
  return (
    <div style={{
      display: "flex",
      justifyContent: "space-between",
      alignItems: "center",
      padding: "8px 0",
      borderBottom: last ? "none" : "1px solid var(--paper-edge)",
    }}>
      <div>
        <div style={{ fontFamily: "var(--serif)", fontSize: 13, color: "var(--ink)" }}>{label}</div>
        <div className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)", marginTop: 2 }}>{via}</div>
      </div>
      <div className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>{time}</div>
    </div>
  );
}

function IntegrationKV({ k, v, last }) {
  return (
    <div style={{
      display: "flex",
      justifyContent: "space-between",
      gap: 12,
      padding: "5px 0",
      borderBottom: last ? "none" : "1px solid var(--paper-edge)",
      fontFamily: "var(--mono)",
      fontSize: 11,
    }}>
      <span style={{ color: "var(--ink-soft)" }}>{k}</span>
      <span style={{ color: "var(--ink)", textAlign: "right" }}>{v}</span>
    </div>
  );
}

window.SyncStatus = SyncStatus;
