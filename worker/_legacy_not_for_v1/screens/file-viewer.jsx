/* global React, ScreenWithChrome, StatusPill, fmtBytes, fmtDateTime */
// File Viewer — 5 variants: normal (with redactions), binary, too_large, ignored, plain.
// Each variant returns a full ScreenWithChrome.

const FILE = () => window.LIBRA_API.file_object__journal;
const REPO = () => window.LIBRA_API.repository;

// ─────────────────────────────────────────────────────────────────────────
// Header strip used inside the viewer body — file metadata
// ─────────────────────────────────────────────────────────────────────────
function FileMetaBar({ file, extra }) {
  return (
    <div style={{
      padding: "20px 28px 18px",
      borderBottom: "1px solid var(--paper-line)",
    }}>
      <div className="lb-eyebrow" style={{ marginBottom: 6 }}>File</div>
      <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: 24 }}>
        <h1 className="lb-h1" style={{ fontFamily: "var(--mono)", fontSize: 22, fontWeight: 500 }}>
          {file.path}
        </h1>
        <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
          <span className="lb-meta">{fmtBytes(file.size_bytes)}</span>
          <span className="lb-meta">·</span>
          <span className="lb-meta">{file.line_count ?? "—"} lines</span>
          <span className="lb-meta">·</span>
          <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-mid)" }}>
            blob {file.blob_sha}
          </span>
        </div>
      </div>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginTop: 12 }}>
        <div style={{ display: "flex", gap: 8 }}>
          {extra}
        </div>
        <div className="lb-meta">
          modified {fmtDateTime(file.last_modified_at)} · {file.last_author}
        </div>
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// View tabs (Source · Versions · Sync)
// ─────────────────────────────────────────────────────────────────────────
function ViewTabs({ active = "source", count }) {
  const items = [
    { id: "source",   label: "Source" },
    { id: "versions", label: `AI Versions${count != null ? ` · ${count}` : ""}` },
    { id: "history",  label: "History" },
    { id: "blame",    label: "Blame" },
  ];
  return (
    <div className="lb-tabs" style={{ padding: "0 28px" }}>
      {items.map(it => (
        <div key={it.id} className={`lb-tab ${it.id === active ? "active" : ""}`}>{it.label}</div>
      ))}
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Notice — flat editorial banner used by binary/too_large/ignored states.
// Never red. Tone is "this isn't displayable, here's why and what to do."
// ─────────────────────────────────────────────────────────────────────────
function Notice({ kind, title, body, fields }) {
  // Color treatments:
  //   info  → ink, paper bg
  //   muted → soft, paper-deep
  //   warn  → gold accent rule
  const tones = {
    info:  { rule: "var(--ink)",  bg: "var(--paper)" },
    muted: { rule: "var(--ink-soft)", bg: "var(--paper-deep)" },
    warn:  { rule: "var(--gold)", bg: "var(--paper)" },
  };
  const t = tones[kind] || tones.info;
  return (
    <div style={{
      flex: 1,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      padding: 40,
      background: t.bg,
    }}>
      <div style={{
        maxWidth: 560,
        borderTop: `2px solid ${t.rule}`,
        paddingTop: 28,
      }}>
        <div className="lb-eyebrow" style={{ color: t.rule, marginBottom: 10 }}>
          {kind === "info" ? "Cannot display inline" :
           kind === "warn" ? "Truncated view" :
           "Excluded from index"}
        </div>
        <h2 style={{
          fontFamily: "var(--serif)",
          fontWeight: 500,
          fontSize: 24,
          color: "var(--ink)",
          margin: "0 0 12px",
          letterSpacing: "-0.005em",
        }}>{title}</h2>
        <p style={{
          fontFamily: "var(--serif)",
          fontSize: 15,
          color: "var(--ink-mid)",
          lineHeight: 1.55,
          margin: "0 0 24px",
          textWrap: "pretty",
        }}>{body}</p>

        {fields && (
          <div style={{
            border: "1px solid var(--paper-line)",
            borderRadius: "var(--r-2)",
            background: "var(--paper)",
            padding: "14px 18px",
            marginBottom: 24,
          }}>
            {fields.map(([k, v], i) => (
              <div key={i} style={{
                display: "flex",
                justifyContent: "space-between",
                gap: 24,
                padding: "5px 0",
                borderBottom: i < fields.length - 1 ? "1px solid var(--paper-edge)" : "none",
                fontFamily: "var(--mono)",
                fontSize: 11.5,
              }}>
                <span style={{ color: "var(--ink-soft)" }}>{k}</span>
                <span style={{ color: "var(--ink)" }}>{v}</span>
              </div>
            ))}
          </div>
        )}

      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Code line + syntax block. Hand-tagged tokens for editorial control.
// ─────────────────────────────────────────────────────────────────────────
function Line({ n, current, children }) {
  return (
    <div style={{
      display: "flex",
      paddingLeft: 24,
      paddingRight: 28,
      background: current ? "rgba(183,135,58,0.08)" : "transparent",
      borderLeft: current ? "2px solid var(--gold)" : "2px solid transparent",
    }}>
      <span className={`lb-ln ${current ? "current" : ""}`}>{n}</span>
      <span style={{ flex: 1 }}>{children}</span>
    </div>
  );
}
const K = ({ children }) => <span className="tok-kw">{children}</span>;
const S = ({ children }) => <span className="tok-str">{children}</span>;
const N = ({ children }) => <span className="tok-num">{children}</span>;
const F = ({ children }) => <span className="tok-fn">{children}</span>;
const C = ({ children }) => <span className="tok-com">{children}</span>;
const P = ({ children }) => <span className="tok-pun">{children}</span>;
const I = ({ children }) => <span className="tok-id">{children}</span>;
const R = ({ w = 24 }) => <span className="lb-redact" style={{ width: w * 6 }}>{"x".repeat(w)}</span>;

// ─────────────────────────────────────────────────────────────────────────
// Source body (the "normal" + "redaction" combined state)
// ─────────────────────────────────────────────────────────────────────────
function SourceBody({ withRedactions = true }) {
  return (
    <div className="lb-code" style={{
      flex: 1,
      overflow: "auto",
      padding: "16px 0",
      background: "var(--paper)",
      borderRight: "1px solid var(--paper-line)",
    }}>
      <Line n={1}><C>// src/ledger/journal.ts — append-only journal</C></Line>
      <Line n={2}><C>// SPDX-License-Identifier: BUSL-1.1</C></Line>
      <Line n={3}>{" "}</Line>
      <Line n={4}><K>import</K> {"{ "}<I>db</I>{", "}<I>signer</I>{" }"} <K>from</K> <S>"../infra"</S><P>;</P></Line>
      <Line n={5}><K>import</K> {"{ "}<I>PostingSchema</I>{" }"} <K>from</K> <S>"./posting"</S><P>;</P></Line>
      <Line n={6}>{" "}</Line>
      <Line n={7}><K>const</K> <I>WEBHOOK_SECRET</I> <P>=</P> <S>"</S>{withRedactions ? <R w={20} /> : <S>whsec_8j2kf91ksldfh23</S>}<S>"</S><P>;</P></Line>
      <Line n={8}><K>const</K> <I>SIGNING_KEY</I> <P>=</P> <I>process</I><P>.</P><I>env</I><P>.</P><I>SIGNING_KEY</I> <P>??</P> <S>"</S>{withRedactions ? <R w={16} /> : <S>sk_live_4kjf83hd8</S>}<S>"</S><P>;</P></Line>
      <Line n={9}>{" "}</Line>
      <Line n={10}><K>export type</K> <F>JournalEntry</F> <P>= {"{"}</P></Line>
      <Line n={11}>{"  "}<I>id</I><P>:</P> <K>string</K><P>;</P></Line>
      <Line n={12}>{"  "}<I>postings</I><P>:</P> <I>PostingSchema</I><P>[];</P></Line>
      <Line n={13}>{"  "}<I>created_at</I><P>:</P> <K>string</K><P>;</P> <C>// ISO-8601</C></Line>
      <Line n={14}>{"  "}<I>idempotency_key</I><P>?:</P> <K>string</K><P>;</P></Line>
      <Line n={15}><P>{"};"}</P></Line>
      <Line n={16}>{" "}</Line>
      <Line n={17}><K>export async function</K> <F>postJournalEntry</F><P>(</P></Line>
      <Line n={18}>{"  "}<I>entry</I><P>:</P> <I>JournalEntry</I><P>,</P></Line>
      <Line n={19}>{"  "}<I>opts</I><P>:</P> {"{ "}<I>retries</I><P>?:</P> <K>number</K> {"}"} <P>= {"{}"},</P></Line>
      <Line n={20}><P>{"): Promise<{ ok: true; sha: string }>"} {"{"}</P></Line>
      <Line n={21} current><span style={{ paddingLeft: 0 }}>{"  "}<K>const</K> <I>sha</I> <P>=</P> <K>await</K> <I>signer</I><P>.</P><F>sign</F><P>(</P><I>entry</I><P>,</P> <I>SIGNING_KEY</I><P>);</P></span></Line>
      <Line n={22}>{"  "}<K>const</K> <I>row</I> <P>=</P> <P>{"{"}</P></Line>
      <Line n={23}>{"    "}<P>...</P><I>entry</I><P>,</P></Line>
      <Line n={24}>{"    "}<I>sha</I><P>,</P></Line>
      <Line n={25}>{"    "}<I>posted_by</I><P>:</P> <S>"</S>{withRedactions ? <R w={14} /> : <S>m.ostrowski@ke</S>}<S>"</S><P>,</P></Line>
      <Line n={26}>{"  "}<P>{"};"}</P></Line>
      <Line n={27}>{"  "}<K>return</K> <I>db</I><P>.</P><F>insert</F><P>(</P><S>"journal"</S><P>,</P> <I>row</I><P>);</P></Line>
      <Line n={28}><P>{"}"}</P></Line>
      <Line n={29}>{" "}</Line>
      <Line n={30}><C>// 154 more lines —</C></Line>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────────
// Right-rail inspector (shows redaction summary, AI versions teaser)
// ─────────────────────────────────────────────────────────────────────────
function Inspector({ file, redactions = [] }) {
  return (
    <aside style={{
      width: 280,
      flexShrink: 0,
      background: "var(--paper-deep)",
      padding: "20px 22px",
      overflow: "auto",
      borderLeft: "1px solid var(--paper-line)",
    }}>
      <Section title="At a glance">
        <KV k="language" v={file.language} />
        <KV k="size_bytes" v={file.size_bytes.toLocaleString()} />
        <KV k="line_count" v={file.line_count} />
        <KV k="has_redactions" v={file.has_redactions ? "true" : "false"} />
        <KV k="redaction_count" v={file.redaction_count} />
      </Section>

      {redactions.length > 0 && (
        <Section title="Redactions">
          <div className="lb-meta" style={{ marginBottom: 10 }}>
            3 spans masked by <span style={{ color: "var(--ink)" }}>secret-scanner</span>.
            Visible only to repo admins with <span className="lb-mono" style={{ color: "var(--ink)" }}>scope=secret.read</span>.
          </div>
          {redactions.map((r, i) => (
            <div key={i} style={{
              padding: "8px 10px",
              background: "var(--paper)",
              border: "1px solid var(--paper-line)",
              borderRadius: "var(--r-1)",
              marginBottom: 6,
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
            }}>
              <div className="lb-mono" style={{ fontSize: 11, color: "var(--ink)" }}>
                line {r.line}
              </div>
              <div className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)" }}>
                {r.rule}
              </div>
            </div>
          ))}
        </Section>
      )}

      <Section title="AI Versions">
        <div className="lb-meta" style={{ marginBottom: 8 }}>2 unreviewed</div>
        <MiniVersion id="av_01H9K2" status="proposed" date="May 07 · 08:42" />
        <MiniVersion id="av_01H9J7" status="accepted" date="May 06 · 19:11" />
      </Section>
    </aside>
  );
}

function Section({ title, children }) {
  return (
    <div style={{ marginBottom: 22 }}>
      <div className="lb-eyebrow" style={{ marginBottom: 10 }}>{title}</div>
      {children}
    </div>
  );
}
function KV({ k, v }) {
  return (
    <div style={{
      display: "flex",
      justifyContent: "space-between",
      padding: "5px 0",
      borderBottom: "1px solid var(--paper-edge)",
      fontFamily: "var(--mono)",
      fontSize: 11,
    }}>
      <span style={{ color: "var(--ink-soft)" }}>{k}</span>
      <span style={{ color: "var(--ink)" }}>{v}</span>
    </div>
  );
}
function MiniVersion({ id, status, date }) {
  return (
    <div style={{
      display: "flex",
      alignItems: "center",
      gap: 10,
      padding: "8px 10px",
      background: "var(--paper)",
      border: "1px solid var(--paper-line)",
      borderRadius: "var(--r-1)",
      marginBottom: 6,
    }}>
      <StatusPill kind={status} />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div className="lb-mono" style={{ fontSize: 10.5, color: "var(--ink)" }}>{id}</div>
        <div className="lb-mono" style={{ fontSize: 10, color: "var(--ink-soft)" }}>{date}</div>
      </div>
    </div>
  );
}

// ═════════════════════════════════════════════════════════════════════════
// SCREENS — five distinct file-viewer states
// ═════════════════════════════════════════════════════════════════════════

function FileViewer_Normal() {
  const file = FILE();
  return (
    <ScreenWithChrome
      active="browse"
      repo={REPO()}
      crumbs={["kepler-ledger", "main", "src", "ledger", "journal.ts"]}
      actions={
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>blob {file.blob_sha}</span>
      }
    >
      <FileMetaBar
        file={file}
        extra={
          <>
            <span className="lb-chip">TypeScript</span>
            <span className="lb-chip dot" style={{ color: "var(--gold)", borderColor: "var(--gold)" }}>
              3 redacted
            </span>
          </>
        }
      />
      <ViewTabs active="source" count={2} />
      <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
        <SourceBody withRedactions />
        <Inspector
          file={file}
          redactions={[
            { line: 7,  rule: "secret.webhook" },
            { line: 8,  rule: "secret.api_key" },
            { line: 25, rule: "pii.email" },
          ]}
        />
      </div>
    </ScreenWithChrome>
  );
}

function FileViewer_Binary() {
  const file = {
    ...FILE(),
    path: "src/ledger/snapshot.bin",
    size_bytes: 4_982_213,
    is_binary: true,
    has_redactions: false,
    redaction_count: 0,
    line_count: null,
    language: "binary",
    blob_sha: "8f0aa12d",
  };
  return (
    <ScreenWithChrome
      active="browse"
      repo={REPO()}
      crumbs={["kepler-ledger", "main", "src", "ledger", "snapshot.bin"]}
      actions={
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>blob {file.blob_sha}</span>
      }
    >
      <FileMetaBar file={file} extra={<><span className="lb-chip">Binary</span></>} />
      <ViewTabs active="source" />
      <Notice
        kind="info"
        title="This file is binary."
        body="Libra does not render binary blobs in the viewer. The raw bytes are still indexed and accessible by blob_sha."
        fields={[
          ["is_binary",   "true"],
          ["mime_type",   "application/octet-stream"],
          ["size_bytes",  file.size_bytes.toLocaleString()],
          ["blob_sha",    file.blob_sha],
        ]}
      />
    </ScreenWithChrome>
  );
}

function FileViewer_TooLarge() {
  const file = {
    ...FILE(),
    path: "logs/2026-04-30.log",
    size_bytes: 38_412_660,
    is_too_large: true,
    line_count: 612_044,
    language: "log",
    blob_sha: "1e2f4ab8",
  };
  return (
    <ScreenWithChrome
      active="browse"
      repo={REPO()}
      crumbs={["kepler-ledger", "main", "logs", "2026-04-30.log"]}
      actions={
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>blob {file.blob_sha}</span>
      }
    >
      <FileMetaBar file={file} extra={<>
        <span className="lb-chip">Log</span>
        <span className="lb-chip" style={{ borderColor: "var(--warn)", color: "var(--warn)" }}>Too large</span>
      </>} />
      <ViewTabs active="source" />
      <Notice
        kind="warn"
        title="File exceeds the 2 MB inline limit."
        body="The viewer truncates beyond is_too_large. Only the first 200 lines are streamed; the remainder remains in raw storage."
        fields={[
          ["is_too_large",   "true"],
          ["size_bytes",     file.size_bytes.toLocaleString()],
          ["viewer_limit",   "2,097,152"],
          ["line_count",     file.line_count.toLocaleString()],
          ["truncated_at",   "200 lines"],
        ]}
      />
    </ScreenWithChrome>
  );
}

function FileViewer_Ignored() {
  const file = {
    ...FILE(),
    path: "vendor/libsecp256k1/secp256k1.c",
    size_bytes: 184_220,
    is_ignored: true,
    line_count: 4_812,
    language: "c",
    blob_sha: "3c19aa7b",
  };
  return (
    <ScreenWithChrome
      active="browse"
      repo={REPO()}
      crumbs={["kepler-ledger", "main", "vendor", "libsecp256k1", "secp256k1.c"]}
      actions={
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>blob {file.blob_sha}</span>
      }
    >
      <FileMetaBar file={file} extra={<>
        <span className="lb-chip">C</span>
        <span className="lb-chip" style={{ color: "var(--ink-soft)" }}>Ignored</span>
      </>} />
      <ViewTabs active="source" />
      <Notice
        kind="muted"
        title="This path is excluded from the index."
        body={
          <>
            Matched by pattern <span className="lb-mono" style={{ color: "var(--ink)" }}>vendor/**</span> in <span className="lb-mono" style={{ color: "var(--ink)" }}>.libraignore</span>. Ignored files do not appear in search results, are not embedded for AI versions, and do not count toward storage.
          </>
        }
        fields={[
          ["is_ignored",        "true"],
          ["matched_pattern",   "vendor/**"],
          ["matched_in",        ".libraignore (line 3)"],
          ["embedded_for_ai",   "false"],
          ["counts_toward_quota", "false"],
        ]}
      />
    </ScreenWithChrome>
  );
}

// Loading state — fifth variant; useful as a counterpoint
function FileViewer_Loading() {
  const file = FILE();
  return (
    <ScreenWithChrome
      active="browse"
      repo={REPO()}
      crumbs={["kepler-ledger", "main", "src", "ledger", "journal.ts"]}
      actions={
        <span className="lb-mono" style={{ fontSize: 11, color: "var(--ink-soft)" }}>fetching…</span>
      }
    >
      <FileMetaBar file={file} extra={<><span className="lb-chip">TypeScript</span></>} />
      <ViewTabs active="source" count={2} />
      <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
        <div style={{ flex: 1, padding: "20px 28px", borderRight: "1px solid var(--paper-line)" }}>
          {[...Array(18)].map((_, i) => (
            <div key={i} style={{ display: "flex", alignItems: "center", gap: 14, padding: "6px 0" }}>
              <div style={{ width: 24, height: 10, background: "var(--paper-edge)", borderRadius: 2 }} />
              <div style={{
                flex: 1,
                height: 10,
                background: "var(--paper-edge)",
                borderRadius: 2,
                width: `${30 + (i * 137) % 60}%`,
              }} />
            </div>
          ))}
        </div>
        <aside style={{ width: 280, background: "var(--paper-deep)", padding: 22, borderLeft: "1px solid var(--paper-line)" }}>
          <div className="lb-eyebrow" style={{ marginBottom: 12 }}>Indexing…</div>
          <div className="lb-meta" style={{ marginBottom: 16, lineHeight: 1.5 }}>
            Computing embeddings for this file. The viewer will show source as soon as the blob is fetched.
          </div>
          {[...Array(3)].map((_, i) => (
            <div key={i} style={{ height: 36, marginBottom: 6, background: "var(--paper)", border: "1px solid var(--paper-line)", borderRadius: 3 }} />
          ))}
        </aside>
      </div>
    </ScreenWithChrome>
  );
}

Object.assign(window, {
  FileViewer_Normal,
  FileViewer_Binary,
  FileViewer_TooLarge,
  FileViewer_Ignored,
  FileViewer_Loading,
});
