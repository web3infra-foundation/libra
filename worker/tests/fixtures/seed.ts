// Helpers to seed the in-memory D1/R2 fixtures with shapes that
// match the tests/data/publish/* JSON contract. Tests hand pick
// what they need.

import type { FakeD1 } from "./fake-d1";
import type { FakeR2 } from "./fake-r2";

const SITE_ID = "00000000-0000-0000-0000-0000publish01";
const REPO_ID = "11111111-2222-3333-4444-555555555555";
const REVISION_OID = "abcdef0123456789abcdef0123456789abcdef01";
const REVISION_OID_DEV = "112233445566778899aabbccddeeff0011223344";
const NOW = "2026-05-09T12:55:00Z";

export const FIXTURE_KEYS = {
  SITE_ID,
  REPO_ID,
  REVISION_OID,
  REVISION_OID_DEV,
} as const;

export type SeedOptions = {
  readonly visibility?: "public" | "private";
  readonly status?: "active" | "disabled";
  readonly cloneDomain?: string;
  readonly slug?: string;
  readonly addAmbiguousRef?: boolean;
};

export async function seedHappyPath(d1: FakeD1, r2: FakeR2, opts: SeedOptions = {}): Promise<void> {
  const visibility = opts.visibility ?? "public";
  const status = opts.status ?? "active";
  const cloneDomain = opts.cloneDomain ?? "code.example.com";
  const slug = opts.slug ?? "libra-demo";

  d1.tables["publish_sync_runs"]!.push({
    sync_run_id: "sync-run-2026-05-09-001",
    site_id: SITE_ID,
    status: "succeeded",
    started_at: "2026-05-09T12:50:00Z",
    finished_at: NOW,
    refs_count: opts.addAmbiguousRef ? 5 : 4,
    revision_count: 2,
    file_count: 5,
    ai_object_count: 4,
    ai_bundle_count: 1,
    warnings_json: "[]",
    error_message: null,
    cli_version: "0.16.3",
    schema_version: 1,
  });

  d1.tables["publish_sites"]!.push({
    site_id: SITE_ID,
    repo_id: REPO_ID,
    clone_domain: cloneDomain,
    slug,
    display_origin: `https://${cloneDomain}`,
    name: "Libra Demo",
    visibility,
    status,
    worker_name: "libra-publish",
    default_ref: "refs/heads/main",
    latest_revision_oid: REVISION_OID,
    refs_generation: 7,
    max_preview_bytes: 1048576,
    schema_version: 1,
    created_at: "2026-05-09T12:00:00Z",
    updated_at: NOW,
  });

  for (const oid of [REVISION_OID, REVISION_OID_DEV]) {
    d1.tables["publish_revisions"]!.push({
      site_id: SITE_ID,
      revision_oid: oid,
      status: "published",
      code_manifest_key: `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${oid}/code-manifest.json`,
      ai_index_key: oid === REVISION_OID
        ? `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${oid}/ai/index.json`
        : null,
      file_count: oid === REVISION_OID ? 5 : 0,
      ai_object_count: oid === REVISION_OID ? 4 : 0,
      ai_bundle_count: oid === REVISION_OID ? 1 : 0,
      redaction_mode: "default",
      redaction_rules_version: "2026.05.09-1",
      sync_run_id: "sync-run-2026-05-09-001",
      schema_version: 1,
      created_at: NOW,
      updated_at: NOW,
    });
  }

  const refs: Array<{ name: string; type: "branch" | "tag"; short: string; target: string; revision: string; isDefault: number }> = [
    { name: "refs/heads/main", type: "branch", short: "main", target: REVISION_OID, revision: REVISION_OID, isDefault: 1 },
    { name: "refs/heads/dev", type: "branch", short: "dev", target: REVISION_OID_DEV, revision: REVISION_OID_DEV, isDefault: 0 },
    { name: "refs/tags/v1.0.0", type: "tag", short: "v1.0.0", target: REVISION_OID, revision: REVISION_OID, isDefault: 0 },
    { name: "refs/tags/v1.1.0-rc", type: "tag", short: "v1.1.0-rc", target: "0011223344556677889900112233445566778899", revision: REVISION_OID_DEV, isDefault: 0 },
  ];
  if (opts.addAmbiguousRef) {
    refs.push({
      name: "refs/tags/dev",
      type: "tag",
      short: "dev",
      target: REVISION_OID_DEV,
      revision: REVISION_OID_DEV,
      isDefault: 0,
    });
  }
  for (const ref of refs) {
    d1.tables["publish_refs"]!.push({
      site_id: SITE_ID,
      ref_name: ref.name,
      ref_type: ref.type,
      short_name: ref.short,
      target_oid: ref.target,
      revision_oid: ref.revision,
      is_default: ref.isDefault,
      sync_run_id: "sync-run-2026-05-09-001",
      schema_version: 1,
      updated_at: NOW,
    });
  }

  // Files for the main revision: text README, src/lib.rs, plus
  // metadata-only entries for binary, too_large and ignored modes.
  const readmeBody = "# Libra demo\n\nHello from publish snapshot.\n";
  const readmeKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/README-fixture.txt`;
  await r2.put(readmeKey, readmeBody);
  const readmeSha = await sha256Hex(readmeBody);

  const libBody = "pub fn libra() -> &'static str { \"libra\" }\n";
  const libKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/files/lib-fixture.txt`;
  await r2.put(libKey, libBody);
  const libSha = await sha256Hex(libBody);

  d1.tables["publish_files"]!.push(
    {
      site_id: SITE_ID,
      revision_oid: REVISION_OID,
      path: "README.md",
      display_mode: "text",
      content_sha256: readmeSha,
      r2_key: readmeKey,
      size_bytes: new TextEncoder().encode(readmeBody).byteLength,
      language: "markdown",
      schema_version: 1,
    },
    {
      site_id: SITE_ID,
      revision_oid: REVISION_OID,
      path: "src/lib.rs",
      display_mode: "text",
      content_sha256: libSha,
      r2_key: libKey,
      size_bytes: new TextEncoder().encode(libBody).byteLength,
      language: "rust",
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

  // AI objects + a bundle.
  const intentKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/ai/objects/snapshot/Intent/intent-2026-05-09-001.json`;
  const intentBody = JSON.stringify({
    schemaVersion: 1,
    objectType: "Intent",
    objectId: "intent-2026-05-09-001",
    layer: "snapshot",
    revisionOid: REVISION_OID,
    sourceRefs: ["refs/heads/main"],
    relationships: [],
    payload: {
      summary: "Publish demo intent",
      providerRawResponse: "sk-public-fixture-1234567890abcdef1234567890",
      absoluteWorkspacePath: "/Users/alice/work/libra",
      nested: {
        visible: "kept",
        promptText: "private system prompt",
        logFile: "/Volumes/Data/GitMono/libra/.libra/log.json",
      },
    },
    redaction: { mode: "default", rulesVersion: "2026.05.09-1" },
    removedFields: ["payload.providerRawResponse", "payload.absoluteWorkspacePath", "payload.promptText"],
  });
  await r2.put(intentKey, intentBody);
  d1.tables["publish_ai_objects"]!.push({
    site_id: SITE_ID,
    revision_oid: REVISION_OID,
    object_type: "Intent",
    object_id: "intent-2026-05-09-001",
    layer: "snapshot",
    r2_key: intentKey,
    redaction_mode: "default",
    payload_sha256: await sha256Hex(intentBody),
    schema_version: 1,
    created_at: NOW,
  });

  const bundleKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/ai/bundles/ai-version-2026-05-09-001.json`;
  const bundleBody = JSON.stringify({
    schemaVersion: 1,
    aiVersionId: "ai-version-2026-05-09-001",
    revisionOid: REVISION_OID,
    objects: [
      { objectType: "Intent", objectId: "intent-2026-05-09-001", layer: "snapshot", r2Key: intentKey },
    ],
    nodes: [
      { objectType: "Intent", objectId: "intent-2026-05-09-001", layer: "snapshot" },
    ],
    edges: [],
    debug: {
      note: "public bundle metadata",
      deploymentToken: "ghp_publicfixture1234567890abcdef",
      absoluteWorkspacePath: "/Users/alice/work/libra",
    },
  });
  await r2.put(bundleKey, bundleBody);
  // Real bundle digest so the Worker's pass-4 P2 verification holds
  // against the in-memory R2 fixture. The seed function computes the
  // sha256 of the bundle body it just wrote to R2.
  const bundleSha = await sha256Hex(bundleBody);
  d1.tables["publish_ai_versions"]!.push({
    site_id: SITE_ID,
    ai_version_id: "ai-version-2026-05-09-001",
    revision_oid: REVISION_OID,
    bundle_key: bundleKey,
    bundle_sha256: bundleSha,
    object_count: 1,
    redaction_mode: "default",
    redaction_rules_version: "2026.05.09-1",
    schema_version: 1,
    created_at: NOW,
  });

  const indexKey = `${REPO_ID}/publish/sites/${SITE_ID}/revisions/${REVISION_OID}/ai/index.json`;
  await r2.put(
    indexKey,
    JSON.stringify({
      schemaVersion: 1,
      bundles: [{ aiVersionId: "ai-version-2026-05-09-001", bundleKey }],
    }),
  );
}

async function sha256Hex(input: string): Promise<string> {
  const data = new TextEncoder().encode(input);
  const buf = await crypto.subtle.digest("SHA-256", data);
  return [...new Uint8Array(buf)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
