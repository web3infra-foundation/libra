import "server-only";
import { PublishApiError } from "./errors";
import type { Bindings } from "./cloudflare";
import type { SiteRow } from "./d1";

const JWKS_PATH = "/cdn-cgi/access/certs";
const JWKS_CACHE_TTL_MS = 60 * 60 * 1000; // 1 hour

type Jwks = {
  readonly keys: ReadonlyArray<{
    readonly kid: string;
    readonly alg?: string;
    readonly kty: string;
    readonly use?: string;
    readonly n?: string;
    readonly e?: string;
  }>;
};

type CachedJwks = { readonly fetchedAt: number; readonly jwks: Jwks };

const jwksCache = new Map<string, CachedJwks>();

/**
 * Enforce site visibility on the incoming request:
 *
 *  - `public` sites: pass through.
 *  - `disabled` sites: caller already returned 410 before reaching here.
 *  - `private` sites: validate the `Cf-Access-Jwt-Assertion` cookie /
 *    header against the team's JWKS, audience tag and issuer.
 *
 * Failing closed (403) is the default whenever the team domain or AUD
 * is not configured: an operator who deploys a private site without
 * Access settings should see denied requests, not silently public
 * content.
 *
 * Codex pass-1 P1: there is intentionally NO bypass env var. An earlier
 * draft accepted `PUBLISH_REQUIRE_ACCESS_FOR_PRIVATE=false` to disable
 * the JWT check for local development; Codex correctly flagged that as
 * a footgun where a misconfigured production deploy could ship private
 * publish data without Access. Private sites now ALWAYS require a
 * valid JWT (or fail closed). For local development against a private
 * site, run Wrangler with a real team-domain + AUD and a Cloudflare
 * Access service token; do not loosen this gate.
 */
export async function enforceVisibility(
  bindings: Bindings,
  request: Request,
  site: SiteRow,
): Promise<void> {
  if (site.visibility !== "private") return;

  const teamDomain = bindings.accessTeamDomain;
  const aud = bindings.accessAud;
  if (!teamDomain || !aud) {
    throw new PublishApiError(
      "ACCESS_REQUIRED",
      403,
      "private site requires Cloudflare Access but the Worker has no team domain / aud configured",
    );
  }

  const token = readAccessJwtFromRequest(request);
  if (!token) {
    throw new PublishApiError(
      "ACCESS_REQUIRED",
      403,
      "missing Cf-Access-Jwt-Assertion header",
    );
  }

  const issuer = `https://${teamDomain}`;
  await verifyAccessJwt(token, issuer, aud);
}

function readAccessJwtFromRequest(request: Request): string | null {
  const headerVal = request.headers.get("Cf-Access-Jwt-Assertion");
  if (headerVal) return headerVal.trim();
  const cookieHeader = request.headers.get("Cookie");
  if (!cookieHeader) return null;
  for (const part of cookieHeader.split(";")) {
    const eq = part.indexOf("=");
    if (eq <= 0) continue;
    const name = part.slice(0, eq).trim();
    if (name === "CF_Authorization") {
      return part.slice(eq + 1).trim();
    }
  }
  return null;
}

async function verifyAccessJwt(
  token: string,
  expectedIssuer: string,
  expectedAud: string,
): Promise<void> {
  // Codex pass-1 P1: every malformed-input error path must surface as
  // 403 ACCESS_DENIED. An earlier draft let `base64UrlToBytes()` /
  // `JSON.parse()` / `crypto.subtle.{importKey,verify}` throw their
  // native `TypeError` / `DOMException`, which would have produced a
  // 500 INTERNAL response and revealed JWT shape information through
  // error messages. Wrap the whole verification path so any throw
  // inside is rewritten as a generic deny.
  try {
    await verifyAccessJwtInner(token, expectedIssuer, expectedAud);
  } catch (error) {
    if (error instanceof PublishApiError) throw error;
    throw deny("malformed Access JWT");
  }
}

async function verifyAccessJwtInner(
  token: string,
  expectedIssuer: string,
  expectedAud: string,
): Promise<void> {
  const parts = token.split(".");
  if (parts.length !== 3) {
    throw deny("malformed Access JWT");
  }
  const [headerB64, payloadB64, signatureB64] = parts as [string, string, string];

  const header = decodeJsonSegment<{ alg: string; kid: string }>(headerB64);
  const payload = decodeJsonSegment<{
    aud?: string | string[];
    iss?: string;
    exp?: number;
    nbf?: number;
  }>(payloadB64);

  if (header.alg !== "RS256") {
    throw deny(`unsupported Access JWT alg: ${header.alg}`);
  }
  if (!header.kid || typeof header.kid !== "string") {
    throw deny("Access JWT is missing kid");
  }
  if (payload.iss !== expectedIssuer) {
    throw deny("Access JWT issuer does not match");
  }
  const audClaim = payload.aud;
  const audMatches = Array.isArray(audClaim)
    ? audClaim.includes(expectedAud)
    : audClaim === expectedAud;
  if (!audMatches) {
    throw deny("Access JWT audience does not match");
  }
  const now = Math.floor(Date.now() / 1000);
  if (typeof payload.exp === "number" && now >= payload.exp) {
    throw deny("Access JWT is expired");
  }
  if (typeof payload.nbf === "number" && now + 60 < payload.nbf) {
    throw deny("Access JWT not yet valid");
  }

  const jwks = await fetchJwks(expectedIssuer);
  const jwk = jwks.keys.find((key) => key.kid === header.kid);
  if (!jwk || !jwk.n || !jwk.e) {
    throw deny("Access JWT kid does not match team JWKS");
  }
  const cryptoKey = await crypto.subtle.importKey(
    "jwk",
    {
      kty: jwk.kty,
      n: jwk.n,
      e: jwk.e,
      alg: "RS256",
      ext: true,
      use: jwk.use ?? "sig",
    } as JsonWebKey,
    { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
    false,
    ["verify"],
  );
  const signed = toArrayBuffer(new TextEncoder().encode(`${headerB64}.${payloadB64}`));
  const sig = toArrayBuffer(base64UrlToBytes(signatureB64));
  const ok = await crypto.subtle.verify("RSASSA-PKCS1-v1_5", cryptoKey, sig, signed);
  if (!ok) throw deny("Access JWT signature verification failed");
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer as ArrayBuffer;
}

function deny(message: string): PublishApiError {
  return new PublishApiError("ACCESS_DENIED", 403, message);
}

function decodeJsonSegment<T>(segment: string): T {
  const bytes = base64UrlToBytes(segment);
  const text = new TextDecoder().decode(bytes);
  try {
    return JSON.parse(text) as T;
  } catch {
    throw deny("malformed Access JWT segment");
  }
}

function base64UrlToBytes(input: string): Uint8Array {
  const padded = input.padEnd(input.length + ((4 - (input.length % 4)) % 4), "=");
  const normalised = padded.replace(/-/g, "+").replace(/_/g, "/");
  const decoded = atob(normalised);
  const bytes = new Uint8Array(decoded.length);
  for (let i = 0; i < decoded.length; i += 1) bytes[i] = decoded.charCodeAt(i);
  return bytes;
}

const JWKS_FETCH_TIMEOUT_MS = 4000;

async function fetchJwks(issuer: string): Promise<Jwks> {
  const cached = jwksCache.get(issuer);
  const now = Date.now();
  if (cached && now - cached.fetchedAt < JWKS_CACHE_TTL_MS) {
    return cached.jwks;
  }
  // Codex pass-3 P2: cap the JWKS fetch so a slow or hung Cloudflare
  // edge can't hold private requests until the platform's hard
  // request timeout. Aborts surface as fail-closed ACCESS_DENIED.
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), JWKS_FETCH_TIMEOUT_MS);
  let response: Response;
  try {
    response = await fetch(`${issuer}${JWKS_PATH}`, {
      signal: controller.signal,
      cf: { cacheEverything: true, cacheTtl: 3600 } as RequestInitCfProperties,
    });
  } catch (error) {
    if (error instanceof Error && error.name === "AbortError") {
      throw deny("Cloudflare Access JWKS fetch timed out");
    }
    throw deny("Cloudflare Access JWKS fetch failed");
  } finally {
    clearTimeout(timeout);
  }
  if (!response.ok) {
    throw deny("Cloudflare Access JWKS fetch failed");
  }
  const body = (await response.json()) as Jwks;
  if (!Array.isArray(body.keys)) {
    throw deny("Cloudflare Access JWKS shape is invalid");
  }
  jwksCache.set(issuer, { fetchedAt: now, jwks: body });
  return body;
}
