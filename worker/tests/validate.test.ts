import { describe, expect, it } from "vitest";
import {
  encodeCursor,
  parseAiVersionId,
  parseCursor,
  parseLayer,
  parseLimit,
  parsePath,
  parsePathOrRoot,
  parseRef,
  parseRepoId,
  parseRevisionOid,
  parseSlug,
} from "@/lib/server/validate";
import { PublishApiError } from "@/lib/server/errors";

const expectBadRequest = (fn: () => unknown) => {
  expect(fn).toThrowError(PublishApiError);
};

describe("validate.parseSlug", () => {
  it("accepts canonical slug", () => {
    expect(parseSlug("libra-demo")).toBe("libra-demo");
  });
  it("rejects empty / leading hyphen / uppercase / oversize", () => {
    expectBadRequest(() => parseSlug(""));
    expectBadRequest(() => parseSlug("-leading"));
    expectBadRequest(() => parseSlug("UPPER"));
    expectBadRequest(() => parseSlug("a".repeat(64)));
  });
});

describe("validate.parseRef", () => {
  it("returns full ref + type for fully-qualified inputs", () => {
    expect(parseRef("refs/heads/main")).toEqual({
      kind: "full",
      fullName: "refs/heads/main",
      type: "branch",
      shortName: "main",
    });
    expect(parseRef("refs/tags/v1.0.0")).toEqual({
      kind: "full",
      fullName: "refs/tags/v1.0.0",
      type: "tag",
      shortName: "v1.0.0",
    });
  });
  it("returns short for unqualified names", () => {
    expect(parseRef("main")).toEqual({ kind: "short", shortName: "main" });
  });
  it("rejects refs/* outside heads/tags", () => {
    expectBadRequest(() => parseRef("refs/remotes/origin/main"));
  });
  it("rejects ref with shell-active characters", () => {
    expectBadRequest(() => parseRef("$(rm -rf /)"));
  });
});

describe("validate.parsePath", () => {
  it("accepts plain paths", () => {
    expect(parsePath("README.md")).toBe("README.md");
    expect(parsePath("src/lib.rs")).toBe("src/lib.rs");
  });
  it("rejects empty", () => {
    expectBadRequest(() => parsePath(""));
  });
  it("rejects traversal & doubled slashes & leading slash & NUL", () => {
    expectBadRequest(() => parsePath("../etc/passwd"));
    expectBadRequest(() => parsePath("a//b"));
    expectBadRequest(() => parsePath("/etc"));
    expectBadRequest(() => parsePath("a\0b"));
  });
});

describe("validate.parsePathOrRoot", () => {
  it("treats null/empty as repo root", () => {
    expect(parsePathOrRoot(null)).toBe("");
    expect(parsePathOrRoot("")).toBe("");
  });
});

describe("validate.parseLayer", () => {
  it("accepts known values", () => {
    expect(parseLayer("snapshot")).toBe("snapshot");
    expect(parseLayer("event")).toBe("event");
    expect(parseLayer("projection")).toBe("projection");
    expect(parseLayer(null)).toBeUndefined();
    expect(parseLayer("")).toBeUndefined();
  });
  it("rejects unknown", () => {
    expectBadRequest(() => parseLayer("foo"));
  });
});

describe("validate.parseLimit", () => {
  it("defaults when absent", () => {
    expect(parseLimit(null)).toBe(50);
  });
  it("rejects out-of-range", () => {
    expectBadRequest(() => parseLimit("0"));
    expectBadRequest(() => parseLimit("-5"));
    expectBadRequest(() => parseLimit("99999"));
  });
});

describe("validate.cursor round-trip", () => {
  it("encodes and decodes a cursor", () => {
    const cursor = { revision: "abcdef" };
    const encoded = encodeCursor(cursor);
    expect(parseCursor(encoded)).toEqual(cursor);
  });
  it("rejects unknown fields", () => {
    const encoded = btoa(JSON.stringify({ foo: "bar" }));
    expectBadRequest(() => parseCursor(encoded));
  });
  it("rejects non-string values", () => {
    const encoded = btoa(JSON.stringify({ revision: 1 }));
    expectBadRequest(() => parseCursor(encoded));
  });
});

describe("validate.parseRepoId / parseRevisionOid / parseAiVersionId", () => {
  it("accepts canonical values", () => {
    expect(parseRepoId("11111111-2222-3333-4444-555555555555")).toBe(
      "11111111-2222-3333-4444-555555555555",
    );
    expect(parseRevisionOid("abcdef0123456789abcdef0123456789abcdef01")).toBe(
      "abcdef0123456789abcdef0123456789abcdef01",
    );
    expect(parseAiVersionId("ai-version-2026-05-09-001")).toBe(
      "ai-version-2026-05-09-001",
    );
  });
  it("rejects malformed input", () => {
    expectBadRequest(() => parseRepoId(""));
    expectBadRequest(() => parseRevisionOid("zzz"));
    expectBadRequest(() => parseRevisionOid("a".repeat(65)));
    expectBadRequest(() => parseAiVersionId("foo bar"));
  });
});
