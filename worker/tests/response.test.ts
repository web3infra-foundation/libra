import { describe, expect, it } from "vitest";
import { respondError, respondOk } from "@/lib/server/response";
import { PublishApiError } from "@/lib/server/errors";

describe("respondOk", () => {
  // Codex pass-2 P1: `respondOk` defaults `visibility` to "private"
  // (fail-safe). Tests that expect public cache headers must opt in
  // explicitly; tests that omit `visibility` exercise the private
  // path and should observe `private, no-store`.
  it("emits envelope + content-type headers", async () => {
    const response = respondOk({ hello: "world" }, { cache: { mode: "no-store" } });
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toMatch(/application\/json/);
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
    const body = await response.json();
    expect(body).toEqual({ ok: true, data: { hello: "world" } });
  });

  it("returns no-store for explicit no-store + public visibility", () => {
    const response = respondOk(
      { x: 1 },
      { cache: { mode: "no-store" }, visibility: "public" },
    );
    expect(response.headers.get("cache-control")).toBe("no-store");
  });

  it("emits revision-long cache headers + ETag for public visibility", () => {
    const response = respondOk(
      { x: 1 },
      { cache: { mode: "revision-long" }, etag: 'W/"x"', visibility: "public" },
    );
    expect(response.headers.get("cache-control")).toMatch(/immutable/);
    expect(response.headers.get("etag")).toBe('W/"x"');
  });

  it("forces private, no-store when visibility is private (overrides cache mode)", () => {
    const response = respondOk(
      { x: 1 },
      { cache: { mode: "revision-long" }, etag: 'W/"x"', visibility: "private" },
    );
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    // ETag intentionally omitted for private responses; intermediaries
    // should never reuse the response across requests.
    expect(response.headers.get("etag")).toBeNull();
  });

  it("defaults to private (fail-safe) when visibility is unset", () => {
    const response = respondOk(
      { x: 1 },
      { cache: { mode: "revision-long" }, etag: 'W/"x"' },
    );
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    expect(response.headers.get("etag")).toBeNull();
  });
});

describe("respondError", () => {
  it("maps PublishApiError to typed JSON", async () => {
    const response = respondError(
      new PublishApiError("REF_NOT_FOUND", 404, "no such ref"),
    );
    expect(response.status).toBe(404);
    expect(response.headers.get("cache-control")).toBe("no-store");
    const body = await response.json();
    expect(body).toEqual({ ok: false, code: "REF_NOT_FOUND", message: "no such ref" });
  });
  it("hides internal errors behind INTERNAL/500", async () => {
    const response = respondError(new Error("oops"));
    expect(response.status).toBe(500);
    const body = await response.json();
    expect(body).toEqual({
      ok: false,
      code: "INTERNAL",
      message: "publish API encountered an internal error",
    });
  });
});
