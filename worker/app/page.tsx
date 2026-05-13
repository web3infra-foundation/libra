export default function Home() {
  return (
    <main className="mx-auto max-w-3xl px-6 py-16">
      <header className="mb-10">
        <span className="lb-eyebrow">Libra publish</span>
        <h1 className="lb-h1 lb-brand mt-3" style={{ fontStyle: "italic", fontSize: 40 }}>
          Libra
        </h1>
        <p className="mt-3 max-w-prose lb-meta" style={{ fontSize: 14 }}>
          Read-only publish endpoint for Libra repositories on Cloudflare
          Workers. Open a site URL to browse code, refs, and the AI object
          model captured in the latest published revision.
        </p>
      </header>

      <section className="lb-card">
        <div className="lb-card-row">
          <div>
            <p className="lb-h2">Browse a published site</p>
            <p className="lb-meta">
              <span className="lb-mono break-all">/sites/&lt;slug&gt;</span>
            </p>
          </div>
          <span className="lb-chip">GET</span>
        </div>
        <div className="lb-card-row">
          <div>
            <p className="lb-h2">Stable repo entry</p>
            <p className="lb-meta">
              <span className="lb-mono break-all">/sites/repo/&lt;repo_id&gt;</span>
            </p>
          </div>
          <span className="lb-chip">GET</span>
        </div>
        <div className="lb-card-row">
          <div>
            <p className="lb-h2">JSON API</p>
            <p className="lb-meta">
              <span className="lb-mono break-all">
                /api/sites/&lt;slug&gt;/&#123;refs|tree|file|ai|status&#125;
              </span>
            </p>
          </div>
          <span className="lb-chip">JSON</span>
        </div>
      </section>

      <section className="mt-10">
        <p className="lb-eyebrow">Restoring the repository locally</p>
        <pre className="lb-codebox mt-3 whitespace-pre-wrap break-all">
{`libra clone libra+cloud://<clone-domain>/<slug>
libra clone libra+cloud://<clone-domain>/repo/<repo_id>
libra clone "libra+cloud://<clone-domain>/<slug>?ref=refs/tags/v1.0.0"`}
        </pre>
        <p className="mt-3 lb-meta lb-meta-soft">
          Worker pages are read-only. Restoring a writable Libra repository
          uses the local CLI and your locally-configured Cloudflare
          credentials — never the Worker URL as a download endpoint.
        </p>
      </section>
    </main>
  );
}
