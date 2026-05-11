// Stub `next/server` for vitest. We only need NextRequest as an
// alias of the platform `Request` type. We re-export the global
// constructor so handlers can `new Request(url, init)` in tests.
export const NextRequest = Request as unknown as typeof Request;
export type NextRequest = Request;
