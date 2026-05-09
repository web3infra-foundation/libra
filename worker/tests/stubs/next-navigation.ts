// Stub `next/navigation` for vitest server-side helpers.
//
// `notFound()` / `forbidden()` / `unauthorized()` / `redirect()`
// throw a tagged error that callers recognise; tests that exercise
// page helpers can either expect the throw or pass slugs that
// resolve successfully.

export class NextRedirectError extends Error {
  readonly target: string;
  constructor(target: string) {
    super(`redirect: ${target}`);
    this.name = "NextRedirectError";
    this.target = target;
  }
}

export class NextNotFoundError extends Error {
  constructor() {
    super("next not-found");
    this.name = "NextNotFoundError";
  }
}

export class NextForbiddenError extends Error {
  constructor() {
    super("next forbidden");
    this.name = "NextForbiddenError";
  }
}

export class NextUnauthorizedError extends Error {
  constructor() {
    super("next unauthorized");
    this.name = "NextUnauthorizedError";
  }
}

export function notFound(): never {
  throw new NextNotFoundError();
}

export function forbidden(): never {
  throw new NextForbiddenError();
}

export function unauthorized(): never {
  throw new NextUnauthorizedError();
}

export function redirect(target: string): never {
  throw new NextRedirectError(target);
}
