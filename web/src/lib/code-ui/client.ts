/**
 * HTTP / SSE client for the Libra Code UI v1 API.
 *
 * All requests target the same-origin embedded server (`/api/code/*`,
 * `/api/repo`, `/api/repo/status`). The Rust side enforces loopback at the
 * server, so this client deliberately does not host-check.
 *
 * Errors are mapped to {@link CodeUiClientError} so callers can switch on
 * `code` (e.g. `CONTROLLER_CONFLICT`, `PAYLOAD_TOO_LARGE`) without parsing
 * raw response bodies.
 */

import type {
  CodeUiAckResponse,
  CodeUiControllerAttachRequest,
  CodeUiControllerAttachResponse,
  CodeUiControllerDetachRequest,
  CodeUiDiagnostics,
  CodeUiErrorEnvelope,
  CodeUiEventEnvelope,
  CodeUiInteractionResponse,
  CodeUiMessageRequest,
  CodeUiSessionSnapshot,
} from "./types";

/** Maximum write-body size accepted by `/api/code/*` write endpoints. */
export const CODE_UI_WRITE_BODY_LIMIT_BYTES = 256 * 1024;

/** Header name carrying the lease token issued by `/controller/attach`. */
export const CONTROLLER_TOKEN_HEADER = "X-Code-Controller-Token";

/** Repository metadata returned by `GET /api/repo`. */
export type RepoInfo = {
  id: string;
  name: string;
  description: string;
};

/** Working-tree status returned by `GET /api/repo/status` (mirrors `libra status --json`). */
export type RepoStatus = {
  head:
    | { type: "branch"; name: string }
    | { type: "detached"; oid: string };
  has_commits: boolean;
  /**
   * `null` when the branch has no upstream, no remote-tracking ref, or the
   * server cannot resolve the remote OID. `ahead` / `behind` are nullable
   * for the same reason — libra serializes them via `Option<usize>`.
   */
  upstream:
    | null
    | {
        remote_ref: string;
        ahead: number | null;
        behind: number | null;
        gone: boolean;
      };
  staged: { new: string[]; modified: string[]; deleted: string[] };
  unstaged: { modified: string[]; deleted: string[] };
  untracked: string[];
  ignored: string[];
  is_clean: boolean;
  stash_entries?: number;
};

/** CLI envelope returned by `libra status --json` and `/api/repo/status`. */
export type RepoStatusEnvelope = {
  ok: true;
  command: "status";
  data: RepoStatus;
};

/** Single row returned by `GET /api/code/threads`. */
export type ThreadListItem = {
  id: string;
  title: string | null;
  archived: boolean;
  currentIntentId: string | null;
  createdAt: string;
  updatedAt: string;
};

export type ThreadListResponse = {
  items: ThreadListItem[];
  nextOffset?: number;
};

export class CodeUiClientError extends Error {
  readonly code: string;
  readonly status: number;
  constructor(code: string, message: string, status: number) {
    super(message);
    this.code = code;
    this.status = status;
    this.name = "CodeUiClientError";
  }
}

async function readJson<T>(response: Response): Promise<T> {
  if (response.ok) {
    return (await response.json()) as T;
  }
  let envelope: CodeUiErrorEnvelope | undefined;
  try {
    envelope = (await response.json()) as CodeUiErrorEnvelope;
  } catch {
    /* fall through to plain status text */
  }
  const code = envelope?.error?.code ?? `HTTP_${response.status}`;
  const message = envelope?.error?.message ?? response.statusText;
  throw new CodeUiClientError(code, message, response.status);
}

export async function getRepoInfo(): Promise<RepoInfo> {
  return readJson<RepoInfo>(
    await fetch("/api/repo", { credentials: "same-origin" }),
  );
}

export async function getRepoStatus(): Promise<RepoStatus> {
  const envelope = await readJson<RepoStatusEnvelope>(
    await fetch("/api/repo/status", { credentials: "same-origin" }),
  );
  return envelope.data;
}

export async function getSession(): Promise<CodeUiSessionSnapshot> {
  return readJson<CodeUiSessionSnapshot>(
    await fetch("/api/code/session", { credentials: "same-origin" }),
  );
}

export async function getDiagnostics(): Promise<CodeUiDiagnostics> {
  return readJson<CodeUiDiagnostics>(
    await fetch("/api/code/diagnostics", { credentials: "same-origin" }),
  );
}

export async function listThreads(
  options: { limit?: number; offset?: number } = {},
): Promise<ThreadListResponse> {
  const params = new URLSearchParams();
  if (options.limit !== undefined) params.set("limit", String(options.limit));
  if (options.offset !== undefined) params.set("offset", String(options.offset));
  const query = params.toString();
  const url = query
    ? `/api/code/threads?${query}`
    : "/api/code/threads";
  return readJson<ThreadListResponse>(
    await fetch(url, { credentials: "same-origin" }),
  );
}

export async function attachController(
  body: CodeUiControllerAttachRequest,
): Promise<CodeUiControllerAttachResponse> {
  return readJson<CodeUiControllerAttachResponse>(
    await fetch("/api/code/controller/attach", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function detachController(
  body: CodeUiControllerDetachRequest,
  controllerToken: string,
): Promise<void> {
  await readJson<{ detached: boolean }>(
    await fetch("/api/code/controller/detach", {
      method: "POST",
      credentials: "same-origin",
      headers: {
        "Content-Type": "application/json",
        [CONTROLLER_TOKEN_HEADER]: controllerToken,
      },
      body: JSON.stringify(body),
    }),
  );
}

export async function submitMessage(
  body: CodeUiMessageRequest,
  controllerToken: string,
): Promise<CodeUiAckResponse> {
  const encoded = JSON.stringify(body);
  if (encoded.length > CODE_UI_WRITE_BODY_LIMIT_BYTES) {
    throw new CodeUiClientError(
      "PAYLOAD_TOO_LARGE",
      "Code UI write request bodies are limited to 256KiB",
      413,
    );
  }
  return readJson<CodeUiAckResponse>(
    await fetch("/api/code/messages", {
      method: "POST",
      credentials: "same-origin",
      headers: {
        "Content-Type": "application/json",
        [CONTROLLER_TOKEN_HEADER]: controllerToken,
      },
      body: encoded,
    }),
  );
}

export async function respondInteraction(
  interactionId: string,
  body: CodeUiInteractionResponse,
  controllerToken: string,
): Promise<CodeUiAckResponse> {
  return readJson<CodeUiAckResponse>(
    await fetch(
      `/api/code/interactions/${encodeURIComponent(interactionId)}`,
      {
        method: "POST",
        credentials: "same-origin",
        headers: {
          "Content-Type": "application/json",
          [CONTROLLER_TOKEN_HEADER]: controllerToken,
        },
        body: JSON.stringify(body),
      },
    ),
  );
}

export async function cancelTurn(controllerToken: string): Promise<CodeUiAckResponse> {
  return readJson<CodeUiAckResponse>(
    await fetch("/api/code/control/cancel", {
      method: "POST",
      credentials: "same-origin",
      headers: { [CONTROLLER_TOKEN_HEADER]: controllerToken },
    }),
  );
}

/**
 * Subscribe to `/api/code/events` SSE.
 *
 * Returns an unsubscribe function. The handler receives parsed
 * {@link CodeUiEventEnvelope}s; payload validation is the caller's job.
 *
 * Reconnect: this helper does NOT auto-retry on its own. The caller is
 * expected to react to `onError` by re-fetching `/api/code/session` and
 * resubscribing — the `Lagged` (broadcast channel saturation) and disconnect
 * cases share the same recovery path.
 */
export function subscribeEvents(
  onEvent: (event: CodeUiEventEnvelope) => void,
  onError?: (error: Event) => void,
): () => void {
  const source = new EventSource("/api/code/events", {
    withCredentials: false,
  });

  const handle = (event: MessageEvent<string>) => {
    if (typeof event.data !== "string") return;
    try {
      const parsed = JSON.parse(event.data) as CodeUiEventEnvelope;
      onEvent(parsed);
    } catch {
      /* server only emits JSON; ignore malformed frames */
    }
  };

  for (const type of ["session_updated", "status_changed", "controller_changed"]) {
    source.addEventListener(type, handle as EventListener);
  }

  if (onError) {
    source.onerror = onError;
  }

  return () => {
    source.close();
  };
}
