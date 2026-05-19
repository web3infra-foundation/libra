// Tiny in-memory R2 mock. Only `get(key)` is exercised by Worker
// reads; writes are handled by the test fixture loader.
type R2Object = {
  readonly key: string;
  readonly body: string;
  readonly size: number;
  readonly etag: string;
  readonly httpEtag: string;
};

export class FakeR2 {
  readonly objects = new Map<string, R2Object>();

  async put(key: string, body: string): Promise<void> {
    const sha = await sha256Hex(body);
    this.objects.set(key, {
      key,
      body,
      size: new TextEncoder().encode(body).byteLength,
      etag: sha,
      httpEtag: `"${sha}"`,
    });
  }

  async get(key: string): Promise<{
    readonly key: string;
    readonly size: number;
    readonly etag: string;
    readonly httpEtag: string;
    text(): Promise<string>;
  } | null> {
    const found = this.objects.get(key);
    if (!found) return null;
    return {
      key: found.key,
      size: found.size,
      etag: found.etag,
      httpEtag: found.httpEtag,
      async text() {
        return found.body;
      },
    };
  }
}

async function sha256Hex(input: string): Promise<string> {
  const data = new TextEncoder().encode(input);
  const buf = await crypto.subtle.digest("SHA-256", data);
  return [...new Uint8Array(buf)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
