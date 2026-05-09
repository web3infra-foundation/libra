import "server-only";
import { PublishApiError, notFound } from "./errors";

const MAX_JSON_BYTES = 8 * 1024 * 1024; // 8 MiB hard cap per JSON read.

/**
 * Read a UTF-8 text file from R2. The caller MUST have already
 * resolved the `r2_key` from a published D1 row — we never accept
 * raw URL parameters here.
 *
 * Returns `null` when the row's `display_mode` indicates there is no
 * R2 content (binary / too_large / ignored). The Worker API
 * surfaces those as metadata-only responses.
 */
export async function readPublishedTextFile(
  bucket: R2Bucket,
  row: { display_mode: string; r2_key: string | null; size_bytes: number },
  expectedSha256?: string | null,
): Promise<{ readonly body: string; readonly etag?: string } | null> {
  if (row.display_mode !== "text" || !row.r2_key) return null;
  const object = await bucket.get(row.r2_key);
  if (!object) {
    // Schema CHECK guarantees text rows carry a non-empty key, so
    // missing R2 here means data inconsistency. We surface a
    // typed corruption error rather than silently empty content.
    throw notFound("R2_OBJECT_MISSING", "published file content is missing");
  }
  if (row.size_bytes > 0 && object.size > row.size_bytes * 4) {
    // Defence-in-depth: catch wildly mismatched sizes before we
    // pull the body into memory. The factor 4 absorbs UTF-8
    // multi-byte expansion vs SQLite's char count if applicable.
    throw new PublishApiError(
      "R2_OBJECT_CORRUPT",
      500,
      "published file size disagrees with index",
    );
  }
  const body = await object.text();
  if (expectedSha256) {
    const actual = await sha256Hex(body);
    if (actual !== expectedSha256) {
      throw new PublishApiError(
        "R2_OBJECT_CORRUPT",
        500,
        "published file content hash does not match index",
      );
    }
  }
  const etag = object.httpEtag ?? object.etag;
  return { body, etag };
}

/**
 * Read a JSON document from R2 by an already-resolved key. Raises
 * a typed error on missing object, oversize body or parse failure.
 */
export async function readPublishedJson<T>(
  bucket: R2Bucket,
  key: string,
): Promise<T> {
  const object = await bucket.get(key);
  if (!object) {
    throw notFound("R2_OBJECT_MISSING", "published JSON object is missing");
  }
  if (object.size > MAX_JSON_BYTES) {
    throw new PublishApiError(
      "R2_OBJECT_CORRUPT",
      500,
      "published JSON object exceeds 8 MiB cap",
    );
  }
  let text: string;
  try {
    text = await object.text();
  } catch {
    throw new PublishApiError("R2_OBJECT_CORRUPT", 500, "published JSON object failed to read");
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new PublishApiError("R2_OBJECT_CORRUPT", 500, "published JSON object is malformed");
  }
}

async function sha256Hex(input: string): Promise<string> {
  const data = new TextEncoder().encode(input);
  const buf = await crypto.subtle.digest("SHA-256", data);
  return [...new Uint8Array(buf)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
