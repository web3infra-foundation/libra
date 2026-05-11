// Stub `@opennextjs/cloudflare` for vitest. Tests inject a fake
// `env` via `setTestEnv()` before invoking handlers.

type TestEnv = {
  ASSETS?: unknown;
  LIBRA_PUBLISH_DB?: unknown;
  LIBRA_PUBLISH_BUCKET?: unknown;
  CF_ACCESS_TEAM_DOMAIN?: string;
  CF_ACCESS_AUD?: string;
};

let testEnv: TestEnv = {};

export function setTestEnv(env: TestEnv): void {
  testEnv = env;
}

export function getCloudflareContext(): { env: TestEnv } {
  return { env: testEnv };
}

export function defineCloudflareConfig<T>(config: T): T {
  return config;
}
