/// <reference types="@cloudflare/vitest-pool-workers" />

import type { D1Migration } from "@cloudflare/vitest-pool-workers/config";
import { applyD1Migrations, env } from "cloudflare:test";
import type { NextRequest } from "next/server";
import { beforeAll, describe, expect, it } from "vitest";
import { setTestEnv } from "./stubs/opennext-cloudflare";

import { GET as siteGet } from "@/app/api/sites/[slug]/route";
import { GET as refsGet } from "@/app/api/sites/[slug]/refs/route";
import { GET as treeGet } from "@/app/api/sites/[slug]/tree/route";
import { GET as fileGet } from "@/app/api/sites/[slug]/file/route";
import { GET as aiVersionsGet } from "@/app/api/sites/[slug]/ai/versions/route";
import { GET as aiObjectsGet } from "@/app/api/sites/[slug]/ai/objects/route";
import { GET as aiObjectGet } from "@/app/api/sites/[slug]/ai/objects/[type]/[id]/route";
import { GET as aiGraphGet } from "@/app/api/sites/[slug]/ai/graph/route";

declare module "cloudflare:test" {
  interface ProvidedEnv {
    readonly LIBRA_PUBLISH_DB: D1Database;
    readonly LIBRA_PUBLISH_BUCKET: R2Bucket;
    readonly TEST_MIGRATIONS: D1Migration[];
    readonly CF_ACCESS_TEAM_DOMAIN: string;
    readonly CF_ACCESS_AUD: string;
  }
}

const HOST = "code.example.com";
const SITE_ID = "00000000-0000-0000-0000-0000publish01";
const REPO_ID = "11111111-2222-3333-4444-555555555555";
const REVISION_OID = "abcdef0123456789abcdef0123456789abcdef01";
const NOW = "2026-05-09T12:55:00Z";

type ApiBody = {
  readonly ok: true;
  readonly data: {
    readonly site: { readonly slug: string };
    readonly refs: readonly { readonly refName: string }[];
    readonly entries: readonly { readonly path: string; readonly entryKind: string }[];
    readonly file: { readonly path: string; readonly displayMode: string };
    readonly content: { readonly body: string } | null;
    readonly versions: readonly { readonly aiVersionId: string }[];
    readonly objects: readonly { readonly objectType: string }[];
    readonly payload: { readonly payload: { readonly summary: string } };
    readonly nodes: readonly { readonly objectType: string }[];
  };
};

beforeAll(async () => {
  setTestEnv({
    LIBRA_PUBLISH_DB: env.LIBRA_PUBLISH_DB,
    LIBRA_PUBLISH_BUCKET: env.LIBRA_PUBLISH_BUCKET,
    CF_ACCESS_TEAM_DOMAIN: env.CF_ACCESS_TEAM_DOMAIN,
    CF_ACCESS_AUD: env.CF_ACCESS_AUD,
  });
  await applyD1Migrations(env.LIBRA_PUBLISH_DB, env.TEST_MIGRATIONS);
  await seedPublishFixture();
});

describe("Miniflare D1/R2 publish API round-trip", () => {
  it("reads site, refs, tree, file and AI data through real D1/R2 bindings", async () => {
    const site = await jsonBody<ApiBody>(
      await siteGet(makeRequest("/api/sites/libra-demo"), {
        params: Promise.resolve({ slug: "libra-demo" }),
      }),
    );
    expect(site.data.site.slug).toBe("libra-demo");

    const refs = await jsonBody<ApiBody>(
      await refsGet(makeRequest("/api/sites/libra-demo/refs"), {
        params: Promise.resolve({ slug: "libra-demo" }),
      }),
    );
    expect(refs.data.refs.map((ref) => ref.refName)).toContain("refs/heads/main");

    const tree = await jsonBody<ApiBody>(
      await treeGet(makeRequest("/api/sites/libra-demo/tree"), {
        params: Promise.resolve({ slug: "libra-demo" }),
      }),
    );
    expect(tree.data.entries).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "README.md", entryKind: "file" }),
        expect.objectContaining({ path: "src", entryKind: "directory" }),
      ]),
    );

    const file = await jsonBody<ApiBody>(
      await fileGet(makeRequest("/api/sites/libra-demo/file?path=README.md"), {
        params: Promise.resolve({ slug: "libra-demo" }),
      }),
    );
    expect(file.data.file.path).toBe("README.md");
    expect(file.data.content?.body).toContain("Hello from Miniflare");

    const versions = await jsonBody<ApiBody>(
      await aiVersionsGet(makeRequest("/api/sites/libra-demo/ai/versions"), {
        params: Promise.resolve({ slug: "libra-demo" }),
      }),
    );
    expect(versions.data.versions[0]?.aiVersionId).toBe("ai-version-miniflare-001");

    const objects = await jsonBody<ApiBody>(
      await aiObjectsGet(makeRequest("/api/sites/libra-demo/ai/objects?type=Intent"), {
        params: Promise.resolve({ slug: "libra-demo" }),
      }),
    );
    expect(objects.data.objects[0]?.objectType).toBe("Intent");

    const objectDetail = await jsonBody<ApiBody>(
      await aiObjectGet(
        makeRequest("/api/sites/libra-demo/ai/objects/Intent/intent-miniflare-001"),
        { params: Promise.resolve({ slug: "libra-demo", type: "Intent", id: "intent-miniflare-001" }) },
      ),
    );
    expect(objectDetail.data.payload.payload.summary).toBe("Miniflare AI intent");

    const graph = await jsonBody<ApiBody>(
      await aiGraphGet(makeRequest("/api/sites/libra-demo/ai/graph"), {
        params: Promise.resolve({ slug: "libra-demo" }),
      }),
    );
    expect(graph.data.nodes).toEqual([
      expect.objectContaining({ objectType: "Intent" }),
    ]);
  });
});

function makeRequest(path: string, init: RequestInit = {}): NextRequest {
  return new Request(`https://${HOST}${path}`, {
    ...init,
    headers: { Host: HOST, ...(init.headers ?? {}) },
  }) as unknown as NextRequest;
}

async function jsonBody<T>(response: Response): Promise<T> {
  expect(response.status).toBe(200);
  return (await response.json()) as T;
}

async function seedPublishFixture(): Promise<void> {
  const db = env.LIBRA_PUBLISH_DB;
  const bucket = env.LIBRA_PUBLISH_BUCKET;
  const readmeBody = "# Libra demo\n\nHello from Miniflare.\n";
  const readmeKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/README.md`;
  await bucket.put(readmeKey, readmeBody);
  const readmeSha = await sha256Hex(readmeBody);

  const libBody = "pub fn libra() -> &'static str { \"libra\" }\n";
  const libKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/src-lib.rs`;
  await bucket.put(libKey, libBody);
  const libSha = await sha256Hex(libBody);

  const intentKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/ai/objects/snapshot/Intent/intent-miniflare-001.json`;
  const intentBody = JSON.stringify({
    schemaVersion: 1,
    siteId: SITE_ID,
    revisionOid: REVISION_OID,
    objectType: "Intent",
    objectId: "intent-miniflare-001",
    layer: "snapshot",
    sourceRefs: ["refs/heads/main"],
    relationships: [],
    payload: { summary: "Miniflare AI intent" },
    redaction: { mode: "default", rulesVersion: "2026.05.09-1" },
    removedFields: [],
  });
  await bucket.put(intentKey, intentBody);
  const intentSha = await sha256Hex(intentBody);

  const bundleKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/ai/bundles/ai-version-miniflare-001.json`;
  const bundleBody = JSON.stringify({
    schemaVersion: 1,
    aiObjectModelReference: "docs/agent/ai-object-model-reference.md",
    siteId: SITE_ID,
    revisionOid: REVISION_OID,
    aiVersionId: "ai-version-miniflare-001",
    objects: [
      {
        objectType: "Intent",
        objectId: "intent-miniflare-001",
        layer: "snapshot",
        r2Key: intentKey,
        payloadSha256: intentSha,
      },
    ],
    relationships: [],
    indexes: {},
    redaction: {
      mode: "default",
      rulesVersion: "2026.05.09-1",
      removedFieldCount: 0,
      removedFieldsByType: {},
      objectCountsByType: { Intent: 1 },
    },
    associatedIds: {},
  });
  await bucket.put(bundleKey, bundleBody);
  const bundleSha = await sha256Hex(bundleBody);

  await db
    .prepare(
      `INSERT INTO publish_sites (
        site_id, repo_id, clone_domain, slug, display_origin, name, visibility, status,
        worker_name, default_ref, latest_revision_oid, refs_generation, max_preview_bytes,
        schema_version, created_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?, ?, ?, ?)`,
    )
    .bind(
      SITE_ID,
      REPO_ID,
      HOST,
      "libra-demo",
      `https://${HOST}`,
      "Libra Demo",
      "public",
      "active",
      "libra-publish",
      1,
      1048576,
      1,
      "2026-05-09T12:00:00Z",
      NOW,
    )
    .run();

  await db
    .prepare(
      `INSERT INTO publish_sync_runs (
        sync_run_id, site_id, status, started_at, finished_at, refs_count, revision_count,
        file_count, ai_object_count, ai_bundle_count, warnings_json, error_message,
        cli_version, schema_version
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)`,
    )
    .bind(
      "sync-run-miniflare-001",
      SITE_ID,
      "succeeded",
      "2026-05-09T12:50:00Z",
      NOW,
      1,
      1,
      2,
      1,
      1,
      "[]",
      "0.17.93",
      1,
    )
    .run();

  await db
    .prepare(
      `INSERT INTO publish_revisions (
        site_id, revision_oid, status, code_manifest_key, ai_index_key, file_count,
        ai_object_count, ai_bundle_count, redaction_mode, redaction_rules_version,
        sync_run_id, schema_version, created_at, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    )
    .bind(
      SITE_ID,
      REVISION_OID,
      "published",
      `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/code-manifest.json`,
      `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/ai/index.json`,
      2,
      1,
      1,
      "default",
      "2026.05.09-1",
      "sync-run-miniflare-001",
      1,
      NOW,
      NOW,
    )
    .run();

  await db
    .prepare(
      `INSERT INTO publish_refs (
        site_id, ref_name, ref_type, short_name, target_oid, revision_oid, is_default,
        sync_run_id, schema_version, updated_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    )
    .bind(
      SITE_ID,
      "refs/heads/main",
      "branch",
      "main",
      REVISION_OID,
      REVISION_OID,
      1,
      "sync-run-miniflare-001",
      1,
      NOW,
    )
    .run();

  await db
    .prepare(
      `UPDATE publish_sites
       SET default_ref = ?, latest_revision_oid = ?, updated_at = ?
       WHERE site_id = ?`,
    )
    .bind("refs/heads/main", REVISION_OID, NOW, SITE_ID)
    .run();

  await db
    .prepare(
      `INSERT INTO publish_files (
        site_id, revision_oid, path, display_mode, content_sha256, r2_key,
        size_bytes, language, schema_version
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?), (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    )
    .bind(
      SITE_ID,
      REVISION_OID,
      "README.md",
      "text",
      readmeSha,
      readmeKey,
      new TextEncoder().encode(readmeBody).byteLength,
      "markdown",
      1,
      SITE_ID,
      REVISION_OID,
      "src/lib.rs",
      "text",
      libSha,
      libKey,
      new TextEncoder().encode(libBody).byteLength,
      "rust",
      1,
    )
    .run();

  await db
    .prepare(
      `INSERT INTO publish_ai_objects (
        site_id, revision_oid, object_type, object_id, layer, r2_key,
        redaction_mode, payload_sha256, schema_version, created_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    )
    .bind(
      SITE_ID,
      REVISION_OID,
      "Intent",
      "intent-miniflare-001",
      "snapshot",
      intentKey,
      "default",
      intentSha,
      1,
      NOW,
    )
    .run();

  await db
    .prepare(
      `INSERT INTO publish_ai_versions (
        site_id, ai_version_id, revision_oid, bundle_key, bundle_sha256,
        object_count, redaction_mode, redaction_rules_version, schema_version, created_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    )
    .bind(
      SITE_ID,
      "ai-version-miniflare-001",
      REVISION_OID,
      bundleKey,
      bundleSha,
      1,
      "default",
      "2026.05.09-1",
      1,
      NOW,
    )
    .run();
}

async function sha256Hex(input: string): Promise<string> {
  const data = new TextEncoder().encode(input);
  const buf = await crypto.subtle.digest("SHA-256", data);
  return [...new Uint8Array(buf)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}
