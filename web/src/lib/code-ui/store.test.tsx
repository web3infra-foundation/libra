// @vitest-environment happy-dom

import { act, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const clientMocks = vi.hoisted(() => ({
  getRepoInfo: vi.fn(),
  getRepoStatus: vi.fn(),
  getSession: vi.fn(),
  listThreads: vi.fn(),
  subscribeEvents: vi.fn(),
}));

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return {
    ...actual,
    getRepoInfo: clientMocks.getRepoInfo,
    getRepoStatus: clientMocks.getRepoStatus,
    getSession: clientMocks.getSession,
    listThreads: clientMocks.listThreads,
    subscribeEvents: clientMocks.subscribeEvents,
  };
});

import { CodeUiClientError, type RepoInfo, type RepoStatus, type ThreadListItem } from "./client";
import { CodeUiProvider, useCodeUiStore, type CodeUiStoreApi } from "./store";
import type { CodeUiEventEnvelope, CodeUiSessionSnapshot } from "./types";

type ActGlobal = typeof globalThis & {
  IS_REACT_ACT_ENVIRONMENT?: boolean;
};

type Subscription = {
  onEvent: (event: CodeUiEventEnvelope) => void;
  onError?: (error: Event) => void;
  unsubscribe: ReturnType<typeof vi.fn>;
};

const repo: RepoInfo = {
  id: "repo-1",
  name: "libra",
  description: "Code UI fixture",
};

const repoStatus: RepoStatus = {
  head: { type: "branch", name: "main" },
  has_commits: true,
  upstream: null,
  staged: { new: [], modified: [], deleted: [] },
  unstaged: { modified: [], deleted: [] },
  untracked: [],
  ignored: [],
  is_clean: true,
};

const thread: ThreadListItem = {
  id: "thread-1",
  title: "Active run",
  archived: false,
  currentIntentId: null,
  createdAt: "2026-05-14T00:00:00Z",
  updatedAt: "2026-05-14T00:01:00Z",
};

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

function renderStore() {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root: Root | null = null;
  let store: CodeUiStoreApi | null = null;

  function Probe({ onStore }: { onStore: (value: CodeUiStoreApi) => void }) {
    const value = useCodeUiStore();
    useEffect(() => {
      onStore(value);
    }, [onStore, value]);
    return null;
  }

  act(() => {
    root = createRoot(container);
    root.render(
      <CodeUiProvider>
        <Probe onStore={(value) => {
          store = value;
        }} />
      </CodeUiProvider>,
    );
  });

  return {
    get store() {
      if (!store) throw new Error("store hook was not rendered");
      return store;
    },
    unmount() {
      act(() => root?.unmount());
      container.remove();
    },
  };
}

async function flushReact() {
  await act(async () => {
    await Promise.resolve();
  });
}

async function waitForStore(
  harness: ReturnType<typeof renderStore>,
  predicate: (store: CodeUiStoreApi) => boolean,
) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    await flushReact();
    if (predicate(harness.store)) return;
  }
  throw new Error(`timed out waiting for store state: ${JSON.stringify(harness.store)}`);
}

let subscriptions: Subscription[] = [];

beforeEach(() => {
  (globalThis as ActGlobal).IS_REACT_ACT_ENVIRONMENT = true;
  subscriptions = [];
  vi.clearAllMocks();
  clientMocks.getRepoInfo.mockResolvedValue(repo);
  clientMocks.getRepoStatus.mockResolvedValue(repoStatus);
  clientMocks.getSession.mockResolvedValue(snapshot("idle"));
  clientMocks.listThreads.mockResolvedValue({ items: [thread] });
  clientMocks.subscribeEvents.mockImplementation((onEvent, onError) => {
    const subscription: Subscription = {
      onEvent,
      onError,
      unsubscribe: vi.fn(),
    };
    subscriptions.push(subscription);
    return subscription.unsubscribe;
  });
});

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("CodeUiProvider", () => {
  it("loads first-paint repo, status, session, and thread state", async () => {
    const harness = renderStore();
    await waitForStore(harness, (store) => store.connection.kind === "ready");

    expect(harness.store.repo).toEqual(repo);
    expect(harness.store.status).toEqual(repoStatus);
    expect(harness.store.snapshot).toMatchObject({ sessionId: "session-1" });
    expect(harness.store.threads).toEqual([thread]);
    expect(subscriptions).toHaveLength(1);

    harness.unmount();
  });

  it("surfaces CODE_UI_UNAVAILABLE without scheduling reconnect", async () => {
    vi.useFakeTimers();
    clientMocks.getSession.mockRejectedValueOnce(
      new CodeUiClientError("CODE_UI_UNAVAILABLE", "Code UI is not running", 503),
    );

    const harness = renderStore();
    await waitForStore(harness, (store) => store.connection.kind === "unavailable");

    expect(harness.store.connection).toMatchObject({
      kind: "unavailable",
      code: "CODE_UI_UNAVAILABLE",
    });
    expect(clientMocks.subscribeEvents).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(15_000));
    expect(clientMocks.getSession).toHaveBeenCalledTimes(1);

    harness.unmount();
  });

  it("backs off and reconnects after an SSE error", async () => {
    vi.useFakeTimers();
    const harness = renderStore();
    await waitForStore(harness, (store) => store.connection.kind === "ready");

    act(() => {
      subscriptions[0].onError?.(new Event("error"));
    });
    expect(subscriptions[0].unsubscribe).toHaveBeenCalledOnce();
    expect(harness.store.connection).toMatchObject({
      kind: "reconnecting",
      attempt: 1,
    });

    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });
    await waitForStore(harness, (store) => store.connection.kind === "ready");

    expect(clientMocks.getSession).toHaveBeenCalledTimes(2);
    expect(subscriptions).toHaveLength(2);

    harness.unmount();
  });

  it("debounces repo status refreshes from bursty SSE events", async () => {
    vi.useFakeTimers();
    const harness = renderStore();
    await waitForStore(harness, (store) => store.connection.kind === "ready");

    act(() => {
      subscriptions[0].onEvent({
        seq: 2,
        type: "session_updated",
        at: "2026-05-14T00:00:02Z",
        data: snapshot("thinking"),
      });
      subscriptions[0].onEvent({
        seq: 3,
        type: "status_changed",
        at: "2026-05-14T00:00:03Z",
        data: snapshot("thinking"),
      });
    });

    expect(clientMocks.getRepoStatus).toHaveBeenCalledTimes(1);
    act(() => vi.advanceTimersByTime(4_999));
    expect(clientMocks.getRepoStatus).toHaveBeenCalledTimes(1);

    await act(async () => {
      vi.advanceTimersByTime(1);
      await Promise.resolve();
    });
    expect(clientMocks.getRepoStatus).toHaveBeenCalledTimes(2);

    harness.unmount();
  });
});
