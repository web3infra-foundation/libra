import Link from "next/link";

export const metadata = {
  title: "Not found · Libra Publish",
};

export default function NotFound() {
  return (
    <main className="mx-auto max-w-2xl px-6 py-24 text-center">
      <p className="libra-pill libra-pill-warn">404</p>
      <h1 className="mt-4 text-2xl font-semibold tracking-tight">
        We could not find that publish target.
      </h1>
      <p className="mt-3 libra-text-muted">
        The site may have been unpublished, the slug may be wrong, or the
        ref / revision is not part of any published snapshot.
      </p>
      <p className="mt-6 text-sm">
        <Link href="/" className="libra-link">
          Back to the publish landing page
        </Link>
      </p>
    </main>
  );
}
