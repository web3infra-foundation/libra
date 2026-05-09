import { describe, expect, it } from "vitest";
import { respondError, respondOk } from "@/lib/server/response";
import { PublishApiError } from "@/lib/server/errors";

describe("respondOk", () => {
  it("emits envelope + cache + content-type headers", async () => {
    const response = respondOk({ hello: "world" }, { cache: { mode: "no-store" } });
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toMatch(/application\/json/);
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(response.headers.get("x-content-type-options")).toBe("nosniff");
    const body = await response.json();
    expect(body).toEqual({ ok: true, data: { hello: "world" } });
  });
  it("emits revision-long cache headers + ETag", () => {
    const response = respondOk({ x: 1 }, { cache: { mode: "revision-long" }, etag: 'W/"x"' });
    expect(response.headers.get("cache-control")).toMatch(/immutable/);
    expect(response.headers.get("etag")).toBe('W/"x"');
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
