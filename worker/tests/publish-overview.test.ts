import { describe, expect, it } from "vitest";
import { loadPublishOverview } from "@/lib/server/d1";
import { publishOverviewToWire } from "@/lib/server/wire";
import { FakeD1 } from "./fixtures/fake-d1";
import { FakeR2 } from "./fixtures/fake-r2";
import { FIXTURE_KEYS, seedHappyPath } from "./fixtures/seed";

describe("loadPublishOverview", () => {
  it("joins refs to revision status and AI version counts", async () => {
    const d1 = new FakeD1();
    const r2 = new FakeR2();
    await seedHappyPath(d1, r2);

    const overview = await loadPublishOverview(
      d1 as unknown as D1Database,
      FIXTURE_KEYS.SITE_ID,
    );

    expect(overview.refs).toHaveLength(4);
    const byName = new Map(overview.refs.map((r) => [r.ref_name, r]));

    const main = byName.get("refs/heads/main");
    expect(main).toBeDefined();
    expect(main?.publish_state).toBe("published");
    expect(main?.file_count).toBe(5);
    // The seed plants exactly one ai_version on the main revision; v1.0.0
    // points at the same revision so it should also report one.
    expect(main?.ai_versions_count).toBe(1);
    expect(byName.get("refs/tags/v1.0.0")?.ai_versions_count).toBe(1);
    // The dev branch and v1.1.0-rc tag share the empty revision; no AI bundle
    // there, so the count must read zero rather than carry main's count.
    expect(byName.get("refs/heads/dev")?.ai_versions_count).toBe(0);
    expect(byName.get("refs/tags/v1.1.0-rc")?.ai_versions_count).toBe(0);

    expect(overview.defaultRef?.ref_name).toBe("refs/heads/main");
  });

  it("returns an empty overview when the site has no refs", async () => {
    const d1 = new FakeD1();
    const overview = await loadPublishOverview(
      d1 as unknown as D1Database,
      "missing-site",
    );
    expect(overview.refs).toEqual([]);
    expect(overview.defaultRef).toBeNull();
  });

  it("chunks IN-list queries to stay under D1's 100-bind cap", async () => {
    // Codex pass-2 P1: D1 caps prepared statements at 100 bound
    // parameters. The implementation chunks the dedup'd revision-oid
    // IN-list at 90 oids per query (leaving slots for site_id and
    // headroom). This regression exercises >99 distinct revisions to
    // confirm the chunking merges results across calls and never
    // binds more than 100 parameters at once.
    const d1 = new FakeD1();
    const siteId = "many-revs-site";
    const distinctRevs = 250;

    for (let i = 0; i < distinctRevs; i++) {
      const oid = `rev-${i.toString().padStart(8, "0")}-${"0".repeat(32)}`;
      d1.tables["publish_revisions"]!.push({
        site_id: siteId,
        revision_oid: oid,
        status: "published",
        code_manifest_key: null,
        ai_index_key: null,
        file_count: i,
        ai_object_count: 0,
        ai_bundle_count: 0,
        redaction_mode: "default",
        redaction_rules_version: "test",
        sync_run_id: "sync-1",
        schema_version: 1,
        created_at: "2026-05-09T12:00:00Z",
        updated_at: "2026-05-09T12:00:00Z",
      });
      d1.tables["publish_refs"]!.push({
        site_id: siteId,
        ref_name: `refs/heads/branch-${i}`,
        ref_type: "branch",
        short_name: `branch-${i}`,
        target_oid: oid,
        revision_oid: oid,
        is_default: i === 0 ? 1 : 0,
        sync_run_id: "sync-1",
        schema_version: 1,
        updated_at: "2026-05-09T12:00:00Z",
      });
      // Plant one AI version on every other revision so the
      // ai-counts merge across chunks is exercised.
      if (i % 2 === 0) {
        d1.tables["publish_ai_versions"]!.push({
          site_id: siteId,
          ai_version_id: `ai-${i}`,
          revision_oid: oid,
          bundle_key: `key-${i}`,
          bundle_sha256: "0".repeat(64),
          object_count: 1,
          redaction_mode: "default",
          redaction_rules_version: "test",
          schema_version: 1,
          created_at: "2026-05-09T12:00:00Z",
        });
      }
    }

    // Wrap `prepare()` to record the largest bind count so we can
    // assert the helper never overshoots D1's 100-bind cap.
    let maxBinds = 0;
    const originalPrepare = d1.prepare.bind(d1);
    d1.prepare = ((sql: string) => {
      const stmt = originalPrepare(sql);
      const originalBind = stmt.bind.bind(stmt);
      stmt.bind = ((...binds: unknown[]) => {
        maxBinds = Math.max(maxBinds, binds.length);
        return originalBind(...(binds as never[]));
      }) as typeof stmt.bind;
      return stmt;
    }) as typeof d1.prepare;

    const overview = await loadPublishOverview(
      d1 as unknown as D1Database,
      siteId,
    );
    expect(overview.refs).toHaveLength(distinctRevs);
    // The first ref is default and revision 0 has an AI version.
    const first = overview.refs.find((r) => r.short_name === "branch-0");
    expect(first?.publish_state).toBe("published");
    expect(first?.ai_versions_count).toBe(1);
    // An odd-index revision should report zero AI versions.
    const odd = overview.refs.find((r) => r.short_name === "branch-1");
    expect(odd?.ai_versions_count).toBe(0);
    // Largest single bind list must include site_id + chunk ≤ 100.
    expect(maxBinds).toBeLessThanOrEqual(100);
  });

  it("converts to wire shape without losing derived fields", async () => {
    const d1 = new FakeD1();
    const r2 = new FakeR2();
    await seedHappyPath(d1, r2);
    const overview = await loadPublishOverview(
      d1 as unknown as D1Database,
      FIXTURE_KEYS.SITE_ID,
    );
    const wire = publishOverviewToWire(overview);
    expect(wire.refs).toHaveLength(overview.refs.length);
    const main = wire.refs.find((r) => r.refName === "refs/heads/main");
    expect(main?.publishState).toBe("published");
    expect(main?.fileCount).toBe(5);
    expect(main?.aiVersionsCount).toBe(1);
    expect(main?.revisionCreatedAt).toBe("2026-05-09T12:55:00Z");
    expect(wire.defaultRef?.refName).toBe("refs/heads/main");
  });
});
