import nextCoreWebVitals from "eslint-config-next/core-web-vitals";
import nextTypescript from "eslint-config-next/typescript";

const config = [
  ...nextCoreWebVitals,
  ...nextTypescript,
  {
    ignores: [
      ".next/**",
      ".open-next/**",
      ".wrangler/**",
      "node_modules/**",
      "cloudflare-env.d.ts",
    ],
  },
  {
    rules: {
      "react-hooks/refs": "off",
      "react-hooks/set-state-in-effect": "off",
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
  // Codex pass-1 P3: enforce the architectural rule the cloudflare
  // helper's docstring promised. React Client Components live under
  // `components/` and any file that opts into the client runtime
  // with `"use client"`. Both must NOT import from
  // `@/lib/server/*` — that path tree is `import "server-only"`,
  // which fails the build only when actually bundled to the
  // browser; the lint rule catches the regression at edit time.
  {
    files: ["components/**/*.{ts,tsx}"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["@/lib/server/*", "**/lib/server/*"],
              message:
                "Client components must not import from `@/lib/server/*`. Move shared types to `@/lib/wire-types` and call the API via `@/lib/client/api`.",
            },
            {
              group: ["server-only"],
              message:
                "`server-only` belongs in `lib/server/*`. Refactor the import out of this client component.",
            },
          ],
        },
      ],
    },
  },
];

export default config;
