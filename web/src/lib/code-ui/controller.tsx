/**
 * Browser controller lease management hook.
 *
 * `useBrowserController()` exposes a lazily-attached lease: the first time the
 * caller invokes `submit()`, `respond()`, or `cancelTurn()` the hook calls
 * `/api/code/controller/attach` with `{ clientId, kind: "browser" }` and
 * caches the returned `controllerToken` in memory only. Reloading the page
 * intentionally drops the lease — the user must re-attach.
 *
 * Recovery semantics:
 * - `MISSING_CONTROLLER_TOKEN` / `INVALID_CONTROLLER_TOKEN` clears the cached
 *   token and retries `attach + send` exactly once.
 * - `CONTROLLER_CONFLICT` surfaces directly; the UI shows the current owner
 *   and stops trying to attach.
 * - `BROWSER_CONTROL_DISABLED` surfaces directly; the UI hints at the
 *   `--browser-control loopback` flag.
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
  attachController,
  cancelTurn,
  CodeUiClientError,
  CONTROLLER_TOKEN_HEADER,
  detachController,
  respondInteraction,
  submitMessage,
} from "./client";
import { useCodeUiStore } from "./store";
import type {
  CodeUiControllerAttachResponse,
  CodeUiInteractionResponse,
} from "./types";

type LeaseState = {
  controllerToken: string;
  leaseExpiresAt: string;
};

export type BrowserControllerStatus =
  | { kind: "idle" }
  | { kind: "attaching" }
  | { kind: "attached"; lease: LeaseState }
  | { kind: "error"; code: string; message: string };

export type BrowserControllerHook = {
  status: BrowserControllerStatus;
  /** Submit a message; lazily attaches if no lease is held. */
  submit: (text: string) => Promise<void>;
  /** Respond to a pending interaction (approval, plan choice, free-text…). */
  respond: (interactionId: string, body: CodeUiInteractionResponse) => Promise<void>;
  /** Cancel the current turn — both browser and automation leases can call this. */
  cancel: () => Promise<void>;
  /** Best-effort detach used by `beforeunload`/`visibilitychange`. */
  detach: () => Promise<void>;
};

function generateClientId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return `browser-${crypto.randomUUID()}`;
  }
  return `browser-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/**
 * Single browser-controller instance shared across the workspace through
 * {@link BrowserControllerProvider}. Mounting `useBrowserController()`
 * twice in the same workspace would create two competing client ids, so any
 * second writer would race the first into `CONTROLLER_CONFLICT`. The provider
 * pattern guarantees one client id and one lease.
 */
const BrowserControllerContext = createContext<BrowserControllerHook | null>(null);

export function BrowserControllerProvider({ children }: { children: ReactNode }) {
  const value = useBrowserControllerInternal();
  return (
    <BrowserControllerContext.Provider value={value}>
      {children}
    </BrowserControllerContext.Provider>
  );
}

export function useBrowserController(): BrowserControllerHook {
  const value = useContext(BrowserControllerContext);
  if (!value) {
    throw new Error(
      "useBrowserController must be used inside <BrowserControllerProvider>",
    );
  }
  return value;
}

function useBrowserControllerInternal(): BrowserControllerHook {
  const { snapshot } = useCodeUiStore();
  const [status, setStatus] = useState<BrowserControllerStatus>({ kind: "idle" });

  // Stable per-mount client identifier. Persists across React re-renders
  // but resets when the page reloads. The `null` initial value is the React
  // idiom for ref-once initialization recommended by `react-hooks/refs`.
  const clientIdRef = useRef<string | null>(null);
  if (clientIdRef.current == null) clientIdRef.current = generateClientId();

  const leaseRef = useRef<LeaseState | null>(null);

  const setAttached = useCallback((attach: CodeUiControllerAttachResponse) => {
    leaseRef.current = {
      controllerToken: attach.controllerToken,
      leaseExpiresAt: attach.leaseExpiresAt,
    };
    setStatus({ kind: "attached", lease: leaseRef.current });
  }, []);

  const ensureLease = useCallback(async (): Promise<LeaseState> => {
    if (leaseRef.current) {
      const expires = Date.parse(leaseRef.current.leaseExpiresAt);
      // Re-attach when within 10s of expiry to absorb clock skew without racing
      // the next write.
      if (Number.isNaN(expires) || expires - Date.now() > 10_000) {
        return leaseRef.current;
      }
    }
    setStatus({ kind: "attaching" });
    try {
      const response = await attachController({
        clientId: clientIdRef.current!,
        kind: "browser",
      });
      setAttached(response);
      return {
        controllerToken: response.controllerToken,
        leaseExpiresAt: response.leaseExpiresAt,
      };
    } catch (error) {
      const apiError =
        error instanceof CodeUiClientError
          ? error
          : new CodeUiClientError("INTERNAL_ERROR", String(error), 500);
      setStatus({ kind: "error", code: apiError.code, message: apiError.message });
      throw apiError;
    }
  }, [setAttached]);

  const withLease = useCallback(
    async <T,>(send: (token: string) => Promise<T>): Promise<T> => {
      const lease = await ensureLease();
      try {
        return await send(lease.controllerToken);
      } catch (error) {
        if (
          error instanceof CodeUiClientError &&
          (error.code === "MISSING_CONTROLLER_TOKEN" ||
            error.code === "INVALID_CONTROLLER_TOKEN" ||
            error.code === "CONTROLLER_CONFLICT")
        ) {
          // Clear the stale lease and try once more.
          leaseRef.current = null;
          setStatus({ kind: "idle" });
          if (error.code === "CONTROLLER_CONFLICT") {
            setStatus({ kind: "error", code: error.code, message: error.message });
            throw error;
          }
          const lease = await ensureLease();
          return send(lease.controllerToken);
        }
        if (error instanceof CodeUiClientError) {
          setStatus({ kind: "error", code: error.code, message: error.message });
        }
        throw error;
      }
    },
    [ensureLease],
  );

  const submit = useCallback(
    async (text: string) => {
      await withLease((token) => submitMessage({ text }, token));
    },
    [withLease],
  );

  const respond = useCallback(
    async (interactionId: string, body: CodeUiInteractionResponse) => {
      await withLease((token) => respondInteraction(interactionId, body, token));
    },
    [withLease],
  );

  const cancel = useCallback(async () => {
    await withLease((token) => cancelTurn(token));
  }, [withLease]);

  const detach = useCallback(async () => {
    const lease = leaseRef.current;
    if (!lease) return;
    leaseRef.current = null;
    setStatus({ kind: "idle" });
    try {
      await detachController({ clientId: clientIdRef.current! }, lease.controllerToken);
    } catch {
      /* best-effort on unload */
    }
  }, []);

  // Best-effort detach on unload / hidden tab so the next browser session can
  // attach without bumping into our stale lease. We use `fetch(..., { keepalive: true })`
  // because the server requires the lease in the `X-Code-Controller-Token`
  // header, and `navigator.sendBeacon` cannot set custom headers.
  useEffect(() => {
    const onUnload = () => {
      const lease = leaseRef.current;
      if (!lease) return;
      try {
        void fetch("/api/code/controller/detach", {
          method: "POST",
          credentials: "same-origin",
          keepalive: true,
          headers: {
            "Content-Type": "application/json",
            [CONTROLLER_TOKEN_HEADER]: lease.controllerToken,
          },
          body: JSON.stringify({ clientId: clientIdRef.current! }),
        });
      } catch {
        /* best-effort on unload */
      }
    };
    window.addEventListener("beforeunload", onUnload);
    return () => window.removeEventListener("beforeunload", onUnload);
  }, []);

  // The store-level `snapshot` lets future iterations react to controller
  // ownership changes (e.g. a TUI reclaim) — the dependency is kept here so
  // the hook re-runs when ownership flips even though there's no direct
  // side-effect to perform yet.
  void snapshot;

  return useMemo(
    () => ({ status, submit, respond, cancel, detach }),
    [status, submit, respond, cancel, detach],
  );
}

export { CONTROLLER_TOKEN_HEADER };
