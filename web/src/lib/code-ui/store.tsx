/**
 * React provider that owns the Libra Code UI runtime state.
 *
 * Lifecycle on mount:
 *   1. Fetch `/api/repo`, `/api/repo/status`, and `/api/code/session` in
 *      parallel for the first paint.
 *   2. Subscribe to `/api/code/events` SSE for incremental updates.
 *   3. On any SSE error / disconnect / `Lagged` (broadcast saturation) on the
 *      Rust side, schedule an exponential-backoff reconnect that always
 *      re-pulls the full session before resubscribing.
 *
 * Controller token + lease metadata live in {@link useCodeUiStore}'s state but
 * are not persisted — refreshing the page intentionally drops the lease.
 */
"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  CodeUiClientError,
  getRepoInfo,
  getRepoStatus,
  getSession,
  subscribeEvents,
  type RepoInfo,
  type RepoStatus,
} from "./client";
import type { CodeUiEventEnvelope, CodeUiSessionSnapshot } from "./types";

type ConnectionState =
  | { kind: "loading" }
  | { kind: "ready" }
  | { kind: "reconnecting"; attempt: number }
  | { kind: "unavailable"; code: string; message: string };

export type CodeUiStoreState = {
  snapshot: CodeUiSessionSnapshot | null;
  repo: RepoInfo | null;
  status: RepoStatus | null;
  connection: ConnectionState;
  lastError: string | null;
};

export type CodeUiStoreApi = CodeUiStoreState & {
  /** Manual refresh — useful for the Summary refresh button. */
  refresh: () => Promise<void>;
  refreshStatus: () => Promise<void>;
};

const CodeUiContext = createContext<CodeUiStoreApi | null>(null);

const RECONNECT_BACKOFF_MS = [500, 1_000, 2_000, 4_000, 8_000, 15_000];

/** Provider component — wrap the workspace in this once at the top level. */
export function CodeUiProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<CodeUiStoreState>({
    snapshot: null,
    repo: null,
    status: null,
    connection: { kind: "loading" },
    lastError: null,
  });

  const reconnectRef = useRef<{ attempt: number; timer: number | null }>({
    attempt: 0,
    timer: null,
  });

  const cancelReconnect = useCallback(() => {
    if (reconnectRef.current.timer != null) {
      window.clearTimeout(reconnectRef.current.timer);
      reconnectRef.current.timer = null;
    }
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      const status = await getRepoStatus();
      setState((s) => ({ ...s, status }));
    } catch (error) {
      // Status endpoint can fail without breaking the chat surface — log only.
      const message = error instanceof Error ? error.message : String(error);
      setState((s) => ({ ...s, lastError: message }));
    }
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [snapshot, status] = await Promise.all([getSession(), getRepoStatus()]);
      setState((s) => ({
        ...s,
        snapshot,
        status,
        connection: { kind: "ready" },
        lastError: null,
      }));
    } catch (error) {
      handleFetchError(error, setState);
    }
  }, []);

  useEffect(() => {
    let unsubscribe: (() => void) | null = null;
    let cancelled = false;

    const reconnect = () => {
      if (cancelled) return;
      const attempt = reconnectRef.current.attempt;
      const delay =
        RECONNECT_BACKOFF_MS[Math.min(attempt, RECONNECT_BACKOFF_MS.length - 1)];
      reconnectRef.current.attempt = attempt + 1;
      setState((s) => ({
        ...s,
        connection: { kind: "reconnecting", attempt: attempt + 1 },
      }));
      reconnectRef.current.timer = window.setTimeout(() => {
        reconnectRef.current.timer = null;
        void connect();
      }, delay);
    };

    const handleEvent = (event: CodeUiEventEnvelope) => {
      // Every Rust event currently carries the full snapshot in `data`.
      if (event.data && typeof event.data === "object") {
        const next = event.data as CodeUiSessionSnapshot;
        setState((s) => ({
          ...s,
          snapshot: next,
          connection: { kind: "ready" },
          lastError: null,
        }));
      }
    };

    const connect = async () => {
      try {
        if (unsubscribe) {
          unsubscribe();
          unsubscribe = null;
        }
        const [repo, status, snapshot] = await Promise.all([
          getRepoInfo().catch(() => null),
          getRepoStatus().catch(() => null),
          getSession(),
        ]);
        if (cancelled) return;
        reconnectRef.current.attempt = 0;
        setState((s) => ({
          ...s,
          snapshot,
          repo: repo ?? s.repo,
          status: status ?? s.status,
          connection: { kind: "ready" },
          lastError: null,
        }));

        unsubscribe = subscribeEvents(handleEvent, () => {
          // SSE error: stop the active stream and back off, then re-fetch
          // the full snapshot before resubscribing — this catches both
          // disconnects and broadcast `Lagged` events on the server.
          if (unsubscribe) {
            unsubscribe();
            unsubscribe = null;
          }
          if (!cancelled) reconnect();
        });
      } catch (error) {
        if (cancelled) return;
        handleFetchError(error, setState);
        if (
          !(error instanceof CodeUiClientError) ||
          error.code !== "CODE_UI_UNAVAILABLE"
        ) {
          reconnect();
        }
      }
    };

    void connect();

    return () => {
      cancelled = true;
      cancelReconnect();
      if (unsubscribe) {
        unsubscribe();
        unsubscribe = null;
      }
    };
  }, [cancelReconnect]);

  const api = useMemo<CodeUiStoreApi>(
    () => ({ ...state, refresh, refreshStatus }),
    [state, refresh, refreshStatus],
  );

  return (
    <CodeUiContext.Provider value={api}>{children}</CodeUiContext.Provider>
  );
}

function handleFetchError(
  error: unknown,
  setState: React.Dispatch<React.SetStateAction<CodeUiStoreState>>,
) {
  if (error instanceof CodeUiClientError && error.code === "CODE_UI_UNAVAILABLE") {
    setState((s) => ({
      ...s,
      snapshot: null,
      connection: { kind: "unavailable", code: error.code, message: error.message },
      lastError: error.message,
    }));
    return;
  }
  const message = error instanceof Error ? error.message : String(error);
  setState((s) => ({
    ...s,
    connection: { kind: "reconnecting", attempt: s.connection.kind === "reconnecting" ? s.connection.attempt : 1 },
    lastError: message,
  }));
}

/** Hook accessor — throws if used outside the provider. */
export function useCodeUiStore(): CodeUiStoreApi {
  const value = useContext(CodeUiContext);
  if (!value) {
    throw new Error("useCodeUiStore must be used within a <CodeUiProvider>");
  }
  return value;
}
