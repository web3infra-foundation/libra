import Link from "next/link";

// Next 16 renders this for `forbidden()` calls (HTTP 403). Pages
// that reach this boundary either lacked a Cloudflare Access JWT or
// presented one we could not validate — the API equivalents are
// `ACCESS_REQUIRED` / `ACCESS_DENIED`.

export const metadata = {
  title: "Access required · Libra Publish",
};

export default function Forbidden() {
  return (
    <main className="mx-auto max-w-2xl px-6 py-24 text-center">
      <span className="lb-chip lb-chip-warn">403</span>
      <h1 className="lb-h1 mt-4">Cloudflare Access is required.</h1>
      <p className="mt-3 lb-meta">
        This site is published with <span className="lb-mono">visibility = private</span>.
        Sign in through the Cloudflare Access application that protects
        this Worker, then reload the page.
      </p>
      <p className="mt-6 text-sm">
        <Link href="/" className="lb-link">
          Back to the publish landing page
        </Link>
      </p>
    </main>
  );
}
