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
