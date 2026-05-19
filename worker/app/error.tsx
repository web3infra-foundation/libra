"use client";

import Link from "next/link";

// Route-level error boundary. Page handlers throw `PublishApiError`
// for typed states (currently `SITE_DISABLED` / `INTERNAL`). The
// boundary inspects the serialized error to render a tailored UX,
// but Next.js bounds the HTTP status code at 500 here — the JSON
// API still emits 410 for disabled sites via /api/*, so monitoring
// can rely on the API status while users see the right message.

type PublishErrorPayload = {
  readonly code?: string;
  readonly httpStatus?: number;
  readonly message?: string;
};

export default function RouteError({
  error,
  reset,
}: {
  readonly error: Error & PublishErrorPayload;
  readonly reset: () => void;
}) {
  const code = error.code ?? "INTERNAL";
  const status = error.httpStatus ?? 500;
  const isDisabled = code === "SITE_DISABLED";

  return (
    <main className="mx-auto max-w-2xl px-6 py-24 text-center">
      <span
        className={
          isDisabled ? "lb-chip lb-chip-warn" : "lb-chip lb-chip-bad"
        }
      >
        {status}
      </span>
      <h1 className="lb-h1 mt-4">
        {isDisabled
          ? "This site has been unpublished."
          : "Something went wrong rendering this page."}
      </h1>
      <p className="mt-3 lb-meta">
        {isDisabled
          ? "The owner unpublished this site. The Worker API returns HTTP 410 for follow-up requests; restoring the repository requires the local Libra CLI."
          : error.message ?? "An unexpected error occurred."}
      </p>
      <p className="mt-6 flex items-center justify-center gap-4 text-sm">
        <Link href="/" className="lb-link">
          Back to the publish landing page
        </Link>
        {!isDisabled && (
          <button type="button" onClick={reset} className="lb-link">
            Retry
          </button>
        )}
      </p>
    </main>
  );
}
