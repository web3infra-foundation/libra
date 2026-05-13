import "server-only";

// All Worker route handlers run on the edge runtime so they get
// access to D1/R2 bindings at the request boundary. Keep this in
// one place so a future Next/edge default change doesn't silently
// drop us into Node-only routes that can't read bindings.
export const runtime = "edge";
export const dynamic = "force-dynamic";
