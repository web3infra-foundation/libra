import "server-only";

const REDACTED = "[redacted]";

const EXACT_SENSITIVE_KEYS = new Set([
  "absoluteworkspacepath",
  "accesstoken",
  "apikey",
  "authorization",
  "credential",
  "password",
  "privatekey",
  "prompttext",
  "providerrawresponse",
  "providerrawtranscript",
  "refreshtoken",
  "secret",
  "secretkey",
  "sessiontoken",
  "sshkey",
  "toolpayload",
  "token",
]);

const SENSITIVE_KEY_FRAGMENTS = ["credential", "password", "secret", "token"];

const SECRET_VALUE_PATTERNS: readonly RegExp[] = [
  /\bsk-[A-Za-z0-9_-]{16,}\b/i,
  /\bgh[pousr]_[A-Za-z0-9_]{16,}\b/i,
  /\bAKIA[0-9A-Z]{16}\b/,
  /\b[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,}\b/,
  /\b(?:token|secret|credential|password)\s*[:=]\s*[^\s"',}]+/i,
];

const LOCAL_PATH_PATTERNS: readonly RegExp[] = [
  /(^|[\s"'`])\/(?:Users|Volumes|home|private|tmp|var|workspace|workspaces)\/[^\s"'`}]*/i,
  /[A-Za-z]:\\(?:Users|Documents and Settings|workspace|workspaces)\\/i,
];

function normalizeKey(key: string): string {
  return key.replace(/[_-]/g, "").toLowerCase();
}

function isSensitiveKey(key: string): boolean {
  const normalized = normalizeKey(key);
  return (
    EXACT_SENSITIVE_KEYS.has(normalized) ||
    SENSITIVE_KEY_FRAGMENTS.some((fragment) => normalized.includes(fragment))
  );
}

function isSensitiveString(value: string): boolean {
  return (
    SECRET_VALUE_PATTERNS.some((pattern) => pattern.test(value)) ||
    LOCAL_PATH_PATTERNS.some((pattern) => pattern.test(value))
  );
}

export function redactPublicAiPayload(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(redactPublicAiPayload);
  }
  if (value && typeof value === "object") {
    const redacted: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
      if (isSensitiveKey(key)) continue;
      redacted[key] = redactPublicAiPayload(child);
    }
    return redacted;
  }
  if (typeof value === "string" && isSensitiveString(value)) {
    return REDACTED;
  }
  return value;
}
