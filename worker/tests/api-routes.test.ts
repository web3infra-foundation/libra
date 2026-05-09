import { beforeEach, describe, expect, it } from "vitest";
import { setTestEnv } from "./stubs/opennext-cloudflare";
import { FakeD1 } from "./fixtures/fake-d1";
import { FakeR2 } from "./fixtures/fake-r2";
import { FIXTURE_KEYS, seedHappyPath } from "./fixtures/seed";

import { GET as siteGet } from "@/app/api/sites/[slug]/route";
import { GET as refsGet } from "@/app/api/sites/[slug]/refs/route";
import { GET as treeGet } from "@/app/api/sites/[slug]/tree/route";
import { GET as fileGet } from "@/app/api/sites/[slug]/file/route";
import { GET as statusGet } from "@/app/api/sites/[slug]/status/route";
import { GET as aiVersionsGet } from "@/app/api/sites/[slug]/ai/versions/route";
import { GET as aiObjectsGet } from "@/app/api/sites/[slug]/ai/objects/route";
import { GET as aiObjectGet } from "@/app/api/sites/[slug]/ai/objects/[type]/[id]/route";
import { GET as aiVersionDetailGet } from "@/app/api/sites/[slug]/ai/versions/[id]/route";
import { GET as aiGraphGet } from "@/app/api/sites/[slug]/ai/graph/route";

const HOST = "code.example.com";

function makeRequest(path: string, init: RequestInit = {}): Request {
  return new Request(`https://${HOST}${path}`, {
    ...init,
    headers: { Host: HOST, ...(init.headers ?? {}) },
  });
}

let d1: FakeD1;
let r2: FakeR2;

beforeEach(async () => {
  d1 = new FakeD1();
  r2 = new FakeR2();
  setTestEnv({
    LIBRA_PUBLISH_DB: d1 as unknown,
    LIBRA_PUBLISH_BUCKET: r2 as unknown,
  });
  await seedHappyPath(d1, r2);
});

describe("/api/sites/[slug]", () => {
  it("returns site + default ref + latest revision", async () => {
    const response = await siteGet(makeRequest("/api/sites/libra-demo"), {
      params: Promise.resolve({ slug: "libra-demo" }),
    });
    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body.ok).toBe(true);
    expect(body.data.site.slug).toBe("libra-demo");
    expect(body.data.defaultRef.refName).toBe("refs/heads/main");
    expect(body.data.latestRevision.revisionOid).toBe(FIXTURE_KEYS.REVISION_OID);
  });
  it("returns 404 for unknown slug", async () => {
    const response = await siteGet(makeRequest("/api/sites/unknown"), {
      params: Promise.resolve({ slug: "unknown" }),
    });
    expect(response.status).toBe(404);
    const body = await response.json();
    expect(body).toMatchObject({ ok: false, code: "SITE_NOT_FOUND" });
  });
  it("returns 410 for disabled site", async () => {
    d1.tables["publish_sites"]![0]!.status = "disabled";
    const response = await siteGet(makeRequest("/api/sites/libra-demo"), {
      params: Promise.resolve({ slug: "libra-demo" }),
    });
    expect(response.status).toBe(410);
    const body = await response.json();
    expect(body).toMatchObject({ ok: false, code: "SITE_DISABLED" });
  });
});

describe("/api/sites/[slug]/refs", () => {
  it("lists refs and respects the type filter", async () => {
    const all = await refsGet(makeRequest("/api/sites/libra-demo/refs"), {
      params: Promise.resolve({ slug: "libra-demo" }),
    });
    const allBody = await all.json();
    expect(allBody.data.refs).toHaveLength(4);
    expect(allBody.data.refsGeneration).toBe(7);

    const branchOnly = await refsGet(
      makeRequest("/api/sites/libra-demo/refs?type=branch"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    const branchBody = await branchOnly.json();
    expect(branchBody.data.refs.every((row: { refType: string }) => row.refType === "branch")).toBe(true);
  });
});

describe("/api/sites/[slug]/tree", () => {
  it("returns root entries for the default ref", async () => {
    const response = await treeGet(makeRequest("/api/sites/libra-demo/tree"), {
      params: Promise.resolve({ slug: "libra-demo" }),
    });
    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body.data.path).toBe("");
    const paths = body.data.entries.map((entry: { path: string; entryKind: string }) => entry.path);
    const kinds = Object.fromEntries(
      body.data.entries.map((entry: { path: string; entryKind: string }) => [entry.path, entry.entryKind]),
    );
    expect(paths).toEqual([".env.local", "README.md", "assets", "src", "tests"]);
    expect(kinds).toEqual({
      ".env.local": "file",
      "README.md": "file",
      "assets": "directory",
      "src": "directory",
      "tests": "directory",
    });
  });
  it("scopes to a sub-directory", async () => {
    const response = await treeGet(makeRequest("/api/sites/libra-demo/tree?path=src"), {
      params: Promise.resolve({ slug: "libra-demo" }),
    });
    const body = await response.json();
    expect(body.data.entries.map((e: { path: string }) => e.path)).toEqual(["src/lib.rs"]);
  });
  it("returns 409 for ambiguous short ref", async () => {
    d1.tables["publish_refs"]!.push({
      site_id: FIXTURE_KEYS.SITE_ID,
      ref_name: "refs/tags/dev",
      ref_type: "tag",
      short_name: "dev",
      target_oid: FIXTURE_KEYS.REVISION_OID_DEV,
      revision_oid: FIXTURE_KEYS.REVISION_OID_DEV,
      is_default: 0,
      sync_run_id: "sync-run-2026-05-09-001",
      schema_version: 1,
      updated_at: "2026-05-09T13:30:00Z",
    });
    const response = await treeGet(
      makeRequest("/api/sites/libra-demo/tree?ref=dev"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    expect(response.status).toBe(409);
    const body = await response.json();
    expect(body).toMatchObject({ ok: false, code: "AMBIGUOUS_REF" });
  });
});

describe("/api/sites/[slug]/file", () => {
  it("returns text content with sha verified", async () => {
    const response = await fileGet(
      makeRequest("/api/sites/libra-demo/file?path=README.md"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body.data.file.path).toBe("README.md");
    expect(body.data.content.body).toContain("Hello from publish snapshot");
  });
  it("returns metadata-only for binary files (no R2 leak)", async () => {
    const response = await fileGet(
      makeRequest("/api/sites/libra-demo/file?path=assets/logo.png"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body.data.file.displayMode).toBe("binary");
    expect(body.data.content).toBeNull();
    expect(JSON.stringify(body)).not.toMatch(/r2_key/);
  });
  it("returns 400 for path traversal", async () => {
    const response = await fileGet(
      makeRequest("/api/sites/libra-demo/file?path=../etc/passwd"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    expect(response.status).toBe(400);
  });
  it("returns 400 when ref and revision are both provided", async () => {
    const response = await fileGet(
      makeRequest(
        `/api/sites/libra-demo/file?path=README.md&ref=refs/heads/main&revision=${FIXTURE_KEYS.REVISION_OID}`,
      ),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    expect(response.status).toBe(400);
  });
});

describe("/api/sites/[slug]/status", () => {
  it("returns the latest sync run", async () => {
    const response = await statusGet(
      makeRequest("/api/sites/libra-demo/status"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body.data.latestSyncRun.status).toBe("succeeded");
    expect(body.data.latestSyncRun.warnings).toEqual([]);
  });
});

describe("/api/sites/[slug]/ai/*", () => {
  it("lists ai versions for the default ref", async () => {
    const response = await aiVersionsGet(
      makeRequest("/api/sites/libra-demo/ai/versions"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body.data.versions[0]?.aiVersionId).toBe("ai-version-2026-05-09-001");
  });
  it("lists ai objects + filters by type and layer", async () => {
    const response = await aiObjectsGet(
      makeRequest("/api/sites/libra-demo/ai/objects?type=Intent&layer=snapshot"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    const body = await response.json();
    expect(body.data.objects).toHaveLength(1);
    expect(body.data.objects[0]?.objectType).toBe("Intent");
  });
  it("returns the AI object payload", async () => {
    const response = await aiObjectGet(
      makeRequest(
        "/api/sites/libra-demo/ai/objects/Intent/intent-2026-05-09-001",
      ),
      { params: Promise.resolve({ slug: "libra-demo", type: "Intent", id: "intent-2026-05-09-001" }) },
    );
    const body = await response.json();
    expect(body.data.payload.payload.summary).toBe("Publish demo intent");
  });

  it("returns AI version detail without storage keys", async () => {
    const response = await aiVersionDetailGet(
      makeRequest("/api/sites/libra-demo/ai/versions/ai-version-2026-05-09-001"),
      { params: Promise.resolve({ slug: "libra-demo", id: "ai-version-2026-05-09-001" }) },
    );
    expect(response.status).toBe(200);
    const body = await response.json();
    expect(body.data.version.aiVersionId).toBe("ai-version-2026-05-09-001");
    expect(body.data.version.bundleSha256).toMatch(/^[0-9a-f]{64}$/);
    // Codex pass-3 P1 + pass-4 P2: r2Key/bundleKey must not leak
    // through the bundle payload, even though they are real fields
    // in the canonical bundle JSON.
    const serialised = JSON.stringify(body.data.bundle);
    expect(serialised).not.toMatch(/r2Key/);
    expect(serialised).not.toMatch(/bundleKey/);
  });

  it("returns the AI graph derived from the canonical bundle", async () => {
    const response = await aiGraphGet(
      makeRequest("/api/sites/libra-demo/ai/graph"),
      { params: Promise.resolve({ slug: "libra-demo" }) },
    );
    expect(response.status).toBe(200);
    const body = await response.json();
    expect(Array.isArray(body.data.nodes)).toBe(true);
    expect(body.data.nodes[0]?.objectType).toBe("Intent");
  });

  it("rejects a tampered bundle (sha mismatch) with R2_OBJECT_CORRUPT", async () => {
    // Overwrite the bundle row's recorded digest so verification
    // fails against the real R2 body — pass-4 P2 must surface a
    // typed 500.
    d1.tables["publish_ai_versions"]![0]!.bundle_sha256 =
      "deadbeef".repeat(8);
    const response = await aiVersionDetailGet(
      makeRequest("/api/sites/libra-demo/ai/versions/ai-version-2026-05-09-001"),
      { params: Promise.resolve({ slug: "libra-demo", id: "ai-version-2026-05-09-001" }) },
    );
    expect(response.status).toBe(500);
    const body = await response.json();
    expect(body).toMatchObject({ ok: false, code: "R2_OBJECT_CORRUPT" });
  });
});

describe("private visibility", () => {
  it("returns 403 when no Access JWT is present", async () => {
    d1.tables["publish_sites"]![0]!.visibility = "private";
    setTestEnv({
      LIBRA_PUBLISH_DB: d1 as unknown,
      LIBRA_PUBLISH_BUCKET: r2 as unknown,
        CF_ACCESS_TEAM_DOMAIN: "team.cloudflareaccess.com",
      CF_ACCESS_AUD: "aud-tag",
    });
    const response = await siteGet(makeRequest("/api/sites/libra-demo"), {
      params: Promise.resolve({ slug: "libra-demo" }),
    });
    expect(response.status).toBe(403);
    const body = await response.json();
    expect(body).toMatchObject({ ok: false, code: "ACCESS_REQUIRED" });
  });
  it("fails closed when Access env is missing", async () => {
    d1.tables["publish_sites"]![0]!.visibility = "private";
    setTestEnv({
      LIBRA_PUBLISH_DB: d1 as unknown,
      LIBRA_PUBLISH_BUCKET: r2 as unknown,
      });
    const response = await siteGet(makeRequest("/api/sites/libra-demo"), {
      params: Promise.resolve({ slug: "libra-demo" }),
    });
    expect(response.status).toBe(403);
  });
});
