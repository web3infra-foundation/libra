# Worker design reference

Static HTML / JSX / CSS package shipped with `docs/improvement/publish.md`
Phase 0 as the visual specification for the Cloudflare Worker frontend
(Phase 6/7).

This directory is a **reference** — it does not run inside the deployed
Worker, is not embedded into the Libra binary, and is excluded from the
`worker/` source-only release manifest. The Phase 6/7 Next.js + OpenNext
implementation under `app/`, `components/`, and `lib/` ports the design
into a code path that actually deploys; this folder lets reviewers
compare the implementation against the original visual mocks without
running a separate static server.

Files:

* `Libra Static Screens.html` — entry HTML, references React 18 +
  Babel-in-the-browser + the JSX bundle below.
* `tokens.css` — paper-navy palette, type stack, radius/shadow tokens.
  These same tokens are mirrored in `worker/app/globals.css` so the
  Tailwind v4 theme stays in lockstep with the design package.
* `core.jsx`, `shell.jsx`, `mock-data.js` — shared shell, layout, and
  sample fixtures.
* `routes/{publish,code,ai}.jsx` — three route screens that map to
  the Next.js pages under `worker/app/sites/[slug]/{,refs,ai,status}`.
* `uploads/*.md` — supporting design / agent-model documents that
  drove the visual choices.

If a future change to the live Worker frontend deviates from the
design package, document the deviation in the PR description (per
publish.md Phase 7 acceptance criteria).
