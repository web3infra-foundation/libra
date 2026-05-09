// Stub `next/headers` for vitest. Tests that need a Host header set
// it through `setTestHeaders()`; default returns an empty Headers.
let testHeaders: Headers = new Headers();

export function setTestHeaders(input: Record<string, string>): void {
  const next = new Headers();
  for (const [key, value] of Object.entries(input)) {
    next.set(key, value);
  }
  testHeaders = next;
}

export async function headers(): Promise<Headers> {
  return testHeaders;
}
