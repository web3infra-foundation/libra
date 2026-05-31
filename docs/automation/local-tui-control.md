# Local TUI Control

> Status: compatibility shim
>
> Canonical user-facing command reference: [`docs/commands/code-control.md`](../commands/code-control.md)
> Current implementation status: `libra code-control --stdio` drives a local Libra Code session over JSON-RPC and loopback `/api/code/*` endpoints.

This document exists to keep historical references from the improvement plans and test docs valid while the control surface is described in the command reference.

## What this file covers

- The local automation control channel used by `libra code --control write`.
- The JSON-RPC stdio shim exposed by `libra code-control --stdio`.
- The endpoint matrix and error-code mapping shared with [`docs/commands/code-control.md`](../commands/code-control.md).

## Where to read the authoritative details

- [`docs/commands/code-control.md`](../commands/code-control.md)
- [`docs/commands/code.md`](../commands/code.md)
- [`docs/improvement/web.md`](../improvement/web.md)

