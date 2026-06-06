# `libra package`

Install, list, and diff **capability packages** — auditable, checksum-verified
bundles of skills, slash-commands, Source Pool sources, and sub-agent
definitions (CEX-S2-17, Step 2.7). This is a Libra-only AI-ecosystem extension,
not a Git command.

## Synopsis

```
libra package list
libra package diff <path>
libra package install <path> [--yes] [--enable]
libra package uninstall <package-id>
```

## Description

A capability package is a local directory containing a `manifest.json` plus the
bundled content files. The manifest declares the package id, version, publisher,
a SHA-256 `checksum` over the bundled content, the `bundled` capabilities
(skills / commands / sources / sub-agents), `requested_permissions`, and
`install_warnings`.

`libra package` is the trust gate for those bundles:

- **`list`** prints every installed package with its version and enabled state,
  read from the per-repo store at `.libra/capability_packages.json`.
- **`diff <path>`** loads a local package and previews the capabilities it would
  grant (new skills / commands / sources / sub-agents / permissions) without
  installing it. A package bundling a new *mutating* capability (a source or
  sub-agent) is flagged as requiring confirmation.
- **`install <path>`** validates the manifest, recomputes and verifies the
  content checksum (a tampered or truncated package is rejected and nothing is
  recorded), computes the capability diff, and records the package. Installs are
  **default-deny**: a package is recorded *disabled* unless `--enable` is passed,
  and a package that grants a new mutating capability — or an update whose
  content checksum changed — requires `--yes` to accept.

Recording a package only persists it to the store; the bundled capabilities are
activated into a live session at session startup from that store, never
implicitly at install time.

## Options

- `--yes` — accept the capability diff without an interactive confirmation
  (required for a package that grants a new mutating capability, or a
  changed-checksum update).
- `--enable` — enable the package immediately instead of leaving it
  installed-but-disabled (default-deny).

## Examples

```
# List what is installed.
libra package list

# Preview what a package would grant before trusting it.
libra package diff ./my-package

# Vet and record a package (prints the capability diff; default-deny disabled).
libra package install ./my-package

# Accept a package that bundles a mutating source/sub-agent, and enable it.
libra package install ./my-package --yes --enable

# Uninstall a recorded package by id.
libra package uninstall acme.toolkit
```

The `- **uninstall**` form drops a package from the per-repo store; its bundled
capabilities (overlap-safe) disappear at the next session start.

## See also

- [`COMPATIBILITY.md`](../../COMPATIBILITY.md) — `package` is a Libra-only
  extension (no Git equivalent).
- `docs/improvement/agent.md` Step 2.7 (CEX-S2-17) — the capability-package /
  plugin-trust design.
