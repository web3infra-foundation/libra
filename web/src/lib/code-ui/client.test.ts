import { afterEach, describe, expect, it, vi } from "vitest";

import {
  CODE_UI_WRITE_BODY_LIMIT_BYTES,
  CONTROLLER_TOKEN_HEADER,
  CodeUiClientError,
  attachController,
  getRepoStatus,
  listThreads,
  submitMessage,
  subscribeEvents,
} from "./client";
import type { CodeUiEventEnvelope, CodeUiSessionSnapshot } from "./types";

function jsonResponse(body: unknown, init: ResponseInit = {}): Response {
  return new Response(JSON.stringify(body), {
    headers: { "Content-Type": "application/json" },
    ...init,
  });
}

function stubFetch(
  handler: (input: RequestInfo | URL, init?: RequestInit) => Response | Promise<Response>,
) {
  const fetchMock = vi.fn(handler);
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

class FakeEventSource {
  static instances: FakeEventSource[] = [];

  readonly listeners = new Map<string, EventListener[]>();
  readonly init?: EventSourceInit;
  closed = false;
  onerror: ((this: EventSource, ev: Event) => unknown) | null = null;

  constructor(readonly url: string | URL, init?: EventSourceInit) {
    this.init = init;
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: EventListener) {
    const existing = this.listeners.get(type) ?? [];
    existing.push(listener);
    this.listeners.set(type, existing);
  }

  close() {
    this.closed = true;
  }

  dispatch(type: string, data: string) {
    const event = { data } as MessageEvent<string>;
    for (const listener of this.listeners.get(type) ?? []) {
      listener.call(this as unknown as EventSource, event);
    }
  }
}

function snapshot(status: CodeUiSessionSnapshot["status"]): CodeUiSessionSnapshot {
  return {
    sessionId: "session-1",
    threadId: "11111111-1111-4111-8111-111111111111",
    workingDir: "/repo",
    provider: { provider: "test", model: "fixture" },
    capabilities: {
      messageInput: true,
      streamingText: true,
      planUpdates: false,
      toolCalls: false,
      patchsets: false,
      interactiveApprovals: false,
      structuredQuestions: false,
      providerSessionResume: false,
    },
    controller: {
      kind: "browser",
      canWrite: true,
      loopbackOnly: true,
      ownerLabel: "browser",
    },
    status,
    transcript: [],
    plans: [],
    tasks: [],
    toolCalls: [],
    patchsets: [],
    interactions: [],
    updatedAt: "2026-05-14T00:00:00Z",
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
  FakeEventSource.instances = [];
});

describe("Code UI HTTP client", () => {
  it("maps server error envelopes into CodeUiClientError", async () => {
    stubFetch(() =>
      jsonResponse(
        {
          error: {
            code: "CONTROLLER_CONFLICT",
            message: "A browser already owns this session",
          },
        },
        { status: 409, statusText: "Conflict" },
      ),
    );

    await expect(
      attachController({ clientId: "browser-tab", kind: "browser" }),
    ).rejects.toMatchObject({
      name: "CodeUiClientError",
      code: "CONTROLLER_CONFLICT",
      status: 409,
      message: "A browser already owns this session",
    });
  });

  it("unwraps RepoStatusEnvelope responses", async () => {
    const status = {
      head: { type: "branch" as const, name: "main" },
      has_commits: true,
      upstream: { remote_ref: "origin/main", ahead: 1, behind: 0, gone: false },
      staged: { new: ["new.txt"], modified: [], deleted: [] },
      unstaged: { modified: ["src/lib.rs"], deleted: [] },
      untracked: ["notes.md"],
      ignored: [],
      is_clean: false,
      stash_entries: 2,
    };
    const fetchMock = stubFetch((input, init) => {
      expect(input).toBe("/api/repo/status");
      expect(init).toMatchObject({ credentials: "same-origin" });
      return jsonResponse({ ok: true, command: "status", data: status });
    });

    await expect(getRepoStatus()).resolves.toEqual(status);
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("encodes thread list pagination as query params", async () => {
    const fetchMock = stubFetch((input, init) => {
      expect(input).toBe("/api/code/threads?limit=50&offset=100");
      expect(init).toMatchObject({ credentials: "same-origin" });
      return jsonResponse({
        items: [
          {
            id: "thread-1",
            title: "Audit run",
            archived: false,
            currentIntentId: null,
            createdAt: "2026-05-14T00:00:00Z",
            updatedAt: "2026-05-14T00:01:00Z",
          },
        ],
        nextOffset: 150,
      });
    });

    await expect(listThreads({ limit: 50, offset: 100 })).resolves.toMatchObject({
      items: [{ id: "thread-1" }],
      nextOffset: 150,
    });
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("sends controller tokens on browser write requests", async () => {
    const fetchMock = stubFetch((input, init) => {
      const headers = new Headers(init?.headers);
      expect(input).toBe("/api/code/messages");
      expect(init?.method).toBe("POST");
      expect(headers.get("Content-Type")).toBe("application/json");
      expect(headers.get(CONTROLLER_TOKEN_HEADER)).toBe("lease-token");
      expect(init?.body).toBe(JSON.stringify({ text: "/chat hello" }));
      return jsonResponse({ accepted: true });
    });

    await expect(submitMessage({ text: "/chat hello" }, "lease-token")).resolves.toEqual({
      accepted: true,
    });
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("rejects oversized browser write bodies before fetch", async () => {
    const fetchMock = stubFetch(() => {
      throw new Error("fetch should not be called for oversized writes");
    });

    await expect(
      submitMessage(
        { text: "x".repeat(CODE_UI_WRITE_BODY_LIMIT_BYTES) },
        "lease-token",
      ),
    ).rejects.toBeInstanceOf(CodeUiClientError);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("Code UI SSE client", () => {
  it("parses known SSE event types and closes subscriptions", () => {
    vi.stubGlobal("EventSource", FakeEventSource);
    const received: CodeUiEventEnvelope[] = [];
    const errors: Event[] = [];

    const unsubscribe = subscribeEvents(
      (event) => received.push(event),
      (error) => errors.push(error),
    );

    const source = FakeEventSource.instances[0];
    expect(source.url).toBe("/api/code/events");
    expect(source.init).toEqual({ withCredentials: false });

    const event = {
      seq: 7,
      type: "session_updated",
      at: "2026-05-14T00:00:00Z",
      data: snapshot("idle"),
    } satisfies CodeUiEventEnvelope;
    source.dispatch("session_updated", JSON.stringify(event));
    source.dispatch("status_changed", "{not-json");
    const error = new Event("error");
    source.onerror?.call(source as unknown as EventSource, error);

    expect(received).toEqual([event]);
    expect(errors).toEqual([error]);

    unsubscribe();
    expect(source.closed).toBe(true);
  });
});
