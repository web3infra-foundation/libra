// Stub `@opennextjs/cloudflare` for vitest. Tests inject a fake
// `env` via `setTestEnv()` before invoking handlers.

import { FakeD1 } from "../fixtures/fake-d1";
import { FakeR2 } from "../fixtures/fake-r2";

type TestEnv = {
  ASSETS?: unknown;
  LIBRA_PUBLISH_DB?: unknown;
  LIBRA_PUBLISH_BUCKET?: unknown;
  CF_ACCESS_TEAM_DOMAIN?: string;
  CF_ACCESS_AUD?: string;
};

const SITE_ID = "00000000-0000-0000-0000-0000publish01";
const REPO_ID = "11111111-2222-3333-4444-555555555555";
const REVISION_OID = "abcdef0123456789abcdef0123456789abcdef01";
const REVISION_OID_DEV = "112233445566778899aabbccddeeff0011223344";
const NOW = "2026-05-09T12:55:00Z";

let testEnv: TestEnv = process.env.LIBRA_PUBLISH_E2E_FIXTURE === "1"
  ? createE2eFixtureEnv()
  : {};

export function setTestEnv(env: TestEnv): void {
  testEnv = env;
}

export function getCloudflareContext(): { env: TestEnv } {
  return { env: testEnv };
}

export function defineCloudflareConfig<T>(config: T): T {
  return config;
}

function createE2eFixtureEnv(): TestEnv {
  const d1 = new FakeD1();
  const r2 = new FakeR2();
  seedE2eFixture(d1, r2);
  return {
    LIBRA_PUBLISH_DB: d1,
    LIBRA_PUBLISH_BUCKET: r2,
  };
}

function seedE2eFixture(db: FakeD1, bucket: FakeR2): void {
  db.tables["publish_sites"]!.push({
    site_id: SITE_ID,
    repo_id: REPO_ID,
    clone_domain: "127.0.0.1",
    slug: "libra-demo",
    display_origin: "http://127.0.0.1:3127",
    name: "Libra Demo",
    visibility: "public",
    status: "active",
    worker_name: "libra-publish",
    default_ref: "refs/heads/main",
    latest_revision_oid: REVISION_OID,
    refs_generation: 7,
    max_preview_bytes: 1048576,
    schema_version: 1,
    created_at: "2026-05-09T12:00:00Z",
    updated_at: NOW,
  });

  db.tables["publish_sync_runs"]!.push({
    sync_run_id: "sync-run-2026-05-09-001",
    site_id: SITE_ID,
    status: "succeeded",
    started_at: "2026-05-09T12:50:00Z",
    finished_at: NOW,
    refs_count: 4,
    revision_count: 2,
    file_count: 6,
    ai_object_count: 1,
    ai_bundle_count: 1,
    warnings_json: "[]",
    error_message: null,
    cli_version: "0.16.3",
    schema_version: 1,
  });

  for (const oid of [REVISION_OID, REVISION_OID_DEV]) {
    db.tables["publish_revisions"]!.push({
      site_id: SITE_ID,
      revision_oid: oid,
      status: "published",
      code_manifest_key: `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${oid}/code-manifest.json`,
      ai_index_key: oid === REVISION_OID
        ? `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${oid}/ai/index.json`
        : null,
      file_count: oid === REVISION_OID ? 6 : 0,
      ai_object_count: oid === REVISION_OID ? 1 : 0,
      ai_bundle_count: oid === REVISION_OID ? 1 : 0,
      redaction_mode: "default",
      redaction_rules_version: "2026.05.09-1",
      sync_run_id: "sync-run-2026-05-09-001",
      schema_version: 1,
      created_at: NOW,
      updated_at: NOW,
    });
  }

  db.tables["publish_refs"]!.push(
    {
      site_id: SITE_ID,
      ref_name: "refs/heads/main",
      ref_type: "branch",
      short_name: "main",
      target_oid: REVISION_OID,
      revision_oid: REVISION_OID,
      is_default: 1,
      sync_run_id: "sync-run-2026-05-09-001",
      schema_version: 1,
      updated_at: NOW,
    },
    {
      site_id: SITE_ID,
      ref_name: "refs/heads/dev",
      ref_type: "branch",
      short_name: "dev",
      target_oid: REVISION_OID_DEV,
      revision_oid: REVISION_OID_DEV,
      is_default: 0,
      sync_run_id: "sync-run-2026-05-09-001",
      schema_version: 1,
      updated_at: NOW,
    },
    {
      site_id: SITE_ID,
      ref_name: "refs/tags/v1.0.0",
      ref_type: "tag",
      short_name: "v1.0.0",
      target_oid: REVISION_OID,
      revision_oid: REVISION_OID,
      is_default: 0,
      sync_run_id: "sync-run-2026-05-09-001",
      schema_version: 1,
      updated_at: NOW,
    },
    {
      site_id: SITE_ID,
      ref_name: "refs/tags/v1.1.0-rc",
      ref_type: "tag",
      short_name: "v1.1.0-rc",
      target_oid: "0011223344556677889900112233445566778899",
      revision_oid: REVISION_OID_DEV,
      is_default: 0,
      sync_run_id: "sync-run-2026-05-09-001",
      schema_version: 1,
      updated_at: NOW,
    },
  );

  putTextObject(
    bucket,
    `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/README-fixture.txt`,
    "# Libra demo\n\nHello from publish snapshot.\n",
    "49dc942731da0588b7a586e55613add0546ed736cd2eb15467e61d814aee53ea",
  );
  putTextObject(
    bucket,
    `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/lib-fixture.txt`,
    "pub fn libra() -> &'static str { \"libra\" }\n",
    "d65bc9f1501008ff920a65d214af51ff9886f739ee55fda8c463e02621805b6a",
  );
  putTextObject(
    bucket,
    `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/long-path-fixture.txt`,
    "export const fixtureComponent = 'long path fixture';\n",
    "536582bf4b90163c623888cd5f6c7732655a179fc2343c1d3e61860d5024600e",
  );

  db.tables["publish_files"]!.push(
    {
      site_id: SITE_ID,
      revision_oid: REVISION_OID,
      path: "README.md",
      display_mode: "text",
      content_sha256: "49dc942731da0588b7a586e55613add0546ed736cd2eb15467e61d814aee53ea",
      r2_key: `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/README-fixture.txt`,
      size_bytes: 42,
      language: "markdown",
      schema_version: 1,
    },
    {
      site_id: SITE_ID,
      revision_oid: REVISION_OID,
      path: "src/lib.rs",
      display_mode: "text",
      content_sha256: "d65bc9f1501008ff920a65d214af51ff9886f739ee55fda8c463e02621805b6a",
      r2_key: `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/lib-fixture.txt`,
      size_bytes: 43,
      language: "rust",
      schema_version: 1,
    },
    {
      site_id: SITE_ID,
      revision_oid: REVISION_OID,
      path: "src/components/really-long-file-name-that-forces-truncation-in-mobile-publish-browser-view.tsx",
      display_mode: "text",
      content_sha256: "536582bf4b90163c623888cd5f6c7732655a179fc2343c1d3e61860d5024600e",
      r2_key: `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/long-path-fixture.txt`,
      size_bytes: 51,
      language: "typescript",
      schema_version: 1,
    },
    {
      site_id: SITE_ID,
      revision_oid: REVISION_OID,
      path: "assets/logo.png",
      display_mode: "binary",
      content_sha256: null,
      r2_key: null,
      size_bytes: 16384,
      language: null,
      schema_version: 1,
    },
    {
      site_id: SITE_ID,
      revision_oid: REVISION_OID,
      path: "tests/data/big-blob.bin",
      display_mode: "too_large",
      content_sha256: null,
      r2_key: null,
      size_bytes: 8388608,
      language: null,
      schema_version: 1,
    },
    {
      site_id: SITE_ID,
      revision_oid: REVISION_OID,
      path: ".env.local",
      display_mode: "ignored",
      content_sha256: null,
      r2_key: null,
      size_bytes: 0,
      language: null,
      schema_version: 1,
    },
  );

  seedE2eAiFixture(db, bucket);
}

function seedE2eAiFixture(db: FakeD1, bucket: FakeR2): void {
  const intentKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/ai/objects/snapshot/Intent/intent-2026-05-09-001.json`;
  const bundleKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/ai/bundles/ai-version-2026-05-09-001.json`;
  const intentBody = JSON.stringify({
    schemaVersion: 1,
    objectType: "Intent",
    objectId: "intent-2026-05-09-001",
    layer: "snapshot",
    revisionOid: REVISION_OID,
    sourceRefs: ["refs/heads/main"],
    relationships: [],
    payload: { summary: "Publish demo intent" },
    redaction: { mode: "default", rulesVersion: "2026.05.09-1" },
    removedFields: [],
  });
  const bundleBody = JSON.stringify({
    schemaVersion: 1,
    aiVersionId: "ai-version-2026-05-09-001",
    revisionOid: REVISION_OID,
    objects: [
      {
        objectType: "Intent",
        objectId: "intent-2026-05-09-001",
        layer: "snapshot",
        r2Key: intentKey,
      },
    ],
    nodes: [{ objectType: "Intent", objectId: "intent-2026-05-09-001", layer: "snapshot" }],
    edges: [],
  });

  putTextObject(
    bucket,
    intentKey,
    intentBody,
    "7ebbf41bd1315af9e1f8731e22d7203056fcdce91a8a853757151ca72bddf790",
  );
  putTextObject(
    bucket,
    bundleKey,
    bundleBody,
    "1a64e9d3b20cb339d2bef89d7116650a6ab930929e2c5599d595615bc08029c4",
  );

  db.tables["publish_ai_objects"]!.push({
    site_id: SITE_ID,
    revision_oid: REVISION_OID,
    object_type: "Intent",
    object_id: "intent-2026-05-09-001",
    layer: "snapshot",
    r2_key: intentKey,
    redaction_mode: "default",
    payload_sha256: "7ebbf41bd1315af9e1f8731e22d7203056fcdce91a8a853757151ca72bddf790",
    schema_version: 1,
    created_at: NOW,
  });
  db.tables["publish_ai_versions"]!.push({
    site_id: SITE_ID,
    ai_version_id: "ai-version-2026-05-09-001",
    revision_oid: REVISION_OID,
    bundle_key: bundleKey,
    bundle_sha256: "1a64e9d3b20cb339d2bef89d7116650a6ab930929e2c5599d595615bc08029c4",
    object_count: 1,
    redaction_mode: "default",
    redaction_rules_version: "2026.05.09-1",
    schema_version: 1,
    created_at: NOW,
  });
}

function putTextObject(bucket: FakeR2, key: string, body: string, sha256: string): void {
  bucket.objects.set(key, {
    key,
    body,
    size: new TextEncoder().encode(body).byteLength,
    etag: sha256,
    httpEtag: `"${sha256}"`,
  });
}
