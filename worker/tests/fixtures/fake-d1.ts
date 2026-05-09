// Tiny in-memory D1 mock backed by a JS object map per table.
// Implements only the prepared-statement subset our handlers use:
//
//   - `SELECT ... FROM <table> WHERE ...` with `?` bind parameters
//   - `INSERT INTO <table> (...) VALUES (...)`
//   - `UPDATE <table> SET ... WHERE ...`
//
// We do **not** ship a SQL parser. Instead, the test fixture
// programmatically populates the rows; we then route SELECTs by
// matching the prepared SQL string. The route handlers in
// `lib/server/d1.ts` use a small, fixed set of statements so this
// mapping stays manageable.

type Row = Record<string, string | number | null>;

export class FakeD1 {
  readonly tables: Record<string, Row[]> = {
    publish_sites: [],
    publish_revisions: [],
    publish_refs: [],
    publish_files: [],
    publish_ai_objects: [],
    publish_ai_versions: [],
    publish_sync_runs: [],
  };

  prepare(sql: string): D1PreparedStatement {
    return new FakePreparedStatement(this, sql);
  }
}

type Bind = string | number | null;

class FakePreparedStatement {
  private readonly db: FakeD1;
  private readonly sql: string;
  private binds: Bind[] = [];

  constructor(db: FakeD1, sql: string) {
    this.db = db;
    this.sql = sql.replace(/\s+/g, " ").trim();
  }

  bind(...binds: Bind[]): this {
    this.binds = binds;
    return this;
  }

  async first<T>(): Promise<T | null> {
    const rows = this.executeSelect();
    return (rows[0] as T | undefined) ?? null;
  }

  async all<T>(): Promise<{ results: T[]; success: true; meta: Record<string, unknown> }> {
    const rows = this.executeSelect();
    return { results: rows as T[], success: true, meta: {} };
  }

  async run(): Promise<{ success: true; meta: Record<string, unknown> }> {
    // Tests insert via fixture helpers, not via raw SQL — we just
    // accept the call so any opportunistic UPDATE in code is a no-op.
    return { success: true, meta: {} };
  }

  private executeSelect(): Row[] {
    const sql = this.sql;
    if (sql.startsWith("SELECT site_id, repo_id, clone_domain")) {
      const [cloneDomain, slug] = this.binds as [string, string?];
      return this.db.tables["publish_sites"]!.filter((row) => {
        if (slug === undefined) {
          return row["slug"] === cloneDomain;
        }
        return row["clone_domain"] === cloneDomain && row["slug"] === slug;
      });
    }
    if (sql.startsWith("SELECT site_id, ref_name, ref_type, short_name") && sql.includes("WHERE site_id = ? AND ref_type = ?")) {
      const [siteId, refType] = this.binds as [string, string];
      return this.db.tables["publish_refs"]!
        .filter((row) => row["site_id"] === siteId && row["ref_type"] === refType)
        .sort((a, b) => sortBy(a, b, ["ref_type", "short_name"]));
    }
    if (sql.startsWith("SELECT site_id, ref_name, ref_type, short_name") && sql.includes("WHERE site_id = ? AND ref_name = ?")) {
      const [siteId, refName] = this.binds as [string, string];
      return this.db.tables["publish_refs"]!.filter(
        (row) => row["site_id"] === siteId && row["ref_name"] === refName,
      );
    }
    if (sql.startsWith("SELECT site_id, ref_name, ref_type, short_name") && sql.includes("WHERE site_id = ? AND short_name = ?")) {
      const [siteId, shortName] = this.binds as [string, string];
      return this.db.tables["publish_refs"]!.filter(
        (row) => row["site_id"] === siteId && row["short_name"] === shortName,
      );
    }
    if (sql.startsWith("SELECT site_id, ref_name, ref_type, short_name") && sql.includes("is_default = 1")) {
      const [siteId] = this.binds as [string];
      return this.db.tables["publish_refs"]!.filter(
        (row) => row["site_id"] === siteId && row["is_default"] === 1,
      );
    }
    if (sql.startsWith("SELECT site_id, ref_name, ref_type, short_name") && sql.includes("WHERE site_id = ? ORDER BY ref_type")) {
      const [siteId] = this.binds as [string];
      return this.db.tables["publish_refs"]!
        .filter((row) => row["site_id"] === siteId)
        .sort((a, b) => sortBy(a, b, ["ref_type", "short_name"]));
    }
    if (sql.startsWith("SELECT site_id, revision_oid, status,") && sql.includes("status = 'published'")) {
      const [siteId, revisionOid] = this.binds as [string, string];
      return this.db.tables["publish_revisions"]!.filter(
        (row) =>
          row["site_id"] === siteId &&
          row["revision_oid"] === revisionOid &&
          row["status"] === "published",
      );
    }
    if (sql.startsWith("SELECT site_id, revision_oid, path") && sql.includes("AND path = ?")) {
      const [siteId, revisionOid, path] = this.binds as [string, string, string];
      return this.db.tables["publish_files"]!.filter(
        (row) =>
          row["site_id"] === siteId &&
          row["revision_oid"] === revisionOid &&
          row["path"] === path,
      );
    }
    if (sql.startsWith("SELECT site_id, revision_oid, path") && sql.includes("AND path >= ? AND path < ?")) {
      const [siteId, revisionOid, lower, upper] = this.binds as [string, string, string, string];
      return this.db.tables["publish_files"]!
        .filter(
          (row) =>
            row["site_id"] === siteId &&
            row["revision_oid"] === revisionOid &&
            (row["path"] as string) >= lower &&
            (row["path"] as string) < upper,
        )
        .sort((a, b) => sortBy(a, b, ["path"]));
    }
    if (sql.startsWith("SELECT site_id, revision_oid, path")) {
      const [siteId, revisionOid] = this.binds as [string, string];
      return this.db.tables["publish_files"]!
        .filter((row) => row["site_id"] === siteId && row["revision_oid"] === revisionOid)
        .sort((a, b) => sortBy(a, b, ["path"]));
    }
    if (sql.startsWith("SELECT site_id, revision_oid, object_type, object_id")) {
      const [siteId, revisionOid] = this.binds as [string, string, ...Bind[]];
      let rows = this.db.tables["publish_ai_objects"]!.filter(
        (row) => row["site_id"] === siteId && row["revision_oid"] === revisionOid,
      );
      // Replicate the conditional WHERE clauses we emit in code.
      const restBinds = this.binds.slice(2);
      if (sql.includes("AND object_type = ? AND object_id = ?")) {
        const [objectType, objectId] = restBinds as [string, string];
        rows = rows.filter((row) => row["object_type"] === objectType && row["object_id"] === objectId);
      } else if (sql.includes("AND object_type = ? AND layer = ?")) {
        const [objectType, layer] = restBinds as [string, string];
        rows = rows.filter((row) => row["object_type"] === objectType && row["layer"] === layer);
      } else if (sql.includes("AND object_type = ?")) {
        const [objectType] = restBinds as [string];
        rows = rows.filter((row) => row["object_type"] === objectType);
      } else if (sql.includes("AND layer = ?")) {
        const [layer] = restBinds as [string];
        rows = rows.filter((row) => row["layer"] === layer);
      }
      return rows.sort((a, b) => sortBy(a, b, ["object_type", "object_id"]));
    }
    if (sql.startsWith("SELECT site_id, ai_version_id, revision_oid")) {
      const [siteId, revisionOid] = this.binds as [string, string];
      return this.db.tables["publish_ai_versions"]!
        .filter((row) => row["site_id"] === siteId && row["revision_oid"] === revisionOid)
        .sort((a, b) => sortBy(a, b, ["ai_version_id"]));
    }
    if (sql.startsWith("SELECT sync_run_id, site_id, status")) {
      const [siteId] = this.binds as [string];
      return this.db.tables["publish_sync_runs"]!
        .filter((row) => row["site_id"] === siteId)
        .sort((a, b) => sortBy(b, a, ["started_at"]));
    }
    throw new Error(`fake-d1: unhandled SQL\n${sql}`);
  }
}

function sortBy(a: Row, b: Row, keys: readonly string[]): number {
  for (const key of keys) {
    const av = a[key];
    const bv = b[key];
    if (av === bv) continue;
    if (av === null) return -1;
    if (bv === null) return 1;
    return av < bv ? -1 : 1;
  }
  return 0;
}
