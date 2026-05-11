import { describe, expect, it } from "vitest";
import { syncRunToWire } from "@/lib/server/wire";

describe("syncRunToWire", () => {
  it("parses warnings_json into a string array", () => {
    const wire = syncRunToWire({
      sync_run_id: "sync-1",
      site_id: "site-1",
      status: "succeeded",
      started_at: "2026-05-09T12:00:00Z",
      finished_at: "2026-05-09T12:05:00Z",
      refs_count: 4,
      revision_count: 2,
      file_count: 5,
      ai_object_count: 4,
      ai_bundle_count: 1,
      warnings_json: '["warning a","warning b"]',
      error_message: null,
      cli_version: "0.16.3",
      schema_version: 1,
    });
    expect(wire.warnings).toEqual(["warning a", "warning b"]);
  });

  it("falls back to [] for malformed JSON without throwing", () => {
    const wire = syncRunToWire({
      sync_run_id: "sync-2",
      site_id: "site-1",
      status: "failed",
      started_at: "2026-05-09T12:00:00Z",
      finished_at: "2026-05-09T12:05:00Z",
      refs_count: 0,
      revision_count: 0,
      file_count: 0,
      ai_object_count: 0,
      ai_bundle_count: 0,
      warnings_json: "not-json",
      error_message: "boom",
      cli_version: "0.16.3",
      schema_version: 1,
    });
    expect(wire.warnings).toEqual([]);
    expect(wire.errorMessage).toBe("boom");
  });

  it("filters non-string entries", () => {
    const wire = syncRunToWire({
      sync_run_id: "sync-3",
      site_id: "site-1",
      status: "running",
      started_at: "2026-05-09T12:00:00Z",
      finished_at: null,
      refs_count: 0,
      revision_count: 0,
      file_count: 0,
      ai_object_count: 0,
      ai_bundle_count: 0,
      warnings_json: '["a", 1, null, "b"]',
      error_message: null,
      cli_version: "0.16.3",
      schema_version: 1,
    });
    expect(wire.warnings).toEqual(["a", "b"]);
  });
});
