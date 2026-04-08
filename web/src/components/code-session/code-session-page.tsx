"use client";

import {
  type FormEvent,
  type ComponentType,
  type ReactNode,
  startTransition,
  useEffect,
  useEffectEvent,
  useState,
} from "react";
import {
  CheckCircle2,
  Clock3,
  FileCode2,
  PencilLine,
  PlugZap,
  ShieldAlert,
  Sparkles,
  TerminalSquare,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

type SessionStatus =
  | "idle"
  | "thinking"
  | "executing_tool"
  | "awaiting_interaction"
  | "completed"
  | "error";

type ControllerKind = "none" | "browser" | "tui" | "cli";
type TranscriptKind =
  | "user_message"
  | "assistant_message"
  | "tool_call"
  | "plan_summary"
  | "diff"
  | "info_note";
type InteractionKind =
  | "approval"
  | "sandbox_approval"
  | "request_user_input"
  | "post_plan_choice";

type CodeUiSessionSnapshot = {
  sessionId: string;
  workingDir: string;
  provider: {
    provider: string;
    model?: string | null;
    mode?: string | null;
    managed: boolean;
  };
  capabilities: {
    messageInput: boolean;
    streamingText: boolean;
    planUpdates: boolean;
    toolCalls: boolean;
    patchsets: boolean;
    interactiveApprovals: boolean;
    structuredQuestions: boolean;
    providerSessionResume: boolean;
  };
  controller: {
    kind: ControllerKind;
    ownerLabel?: string | null;
    canWrite: boolean;
    leaseExpiresAt?: string | null;
    reason?: string | null;
    loopbackOnly: boolean;
  };
  status: SessionStatus;
  transcript: Array<{
    id: string;
    kind: TranscriptKind;
    title?: string | null;
    content?: string | null;
    status?: string | null;
    streaming: boolean;
    metadata: Record<string, unknown>;
    createdAt: string;
    updatedAt: string;
  }>;
  plans: Array<{
    id: string;
    title?: string | null;
    summary?: string | null;
    status: string;
    steps: Array<{ step: string; status: string }>;
    updatedAt: string;
  }>;
  tasks: Array<{
    id: string;
    title?: string | null;
    status: string;
    details?: string | null;
    updatedAt: string;
  }>;
  toolCalls: Array<{
    id: string;
    toolName: string;
    status: string;
    summary?: string | null;
    details?: string | null;
    updatedAt: string;
  }>;
  patchsets: Array<{
    id: string;
    status: string;
    changes: Array<{
      path: string;
      changeType: string;
      diff?: string | null;
    }>;
    updatedAt: string;
  }>;
  interactions: Array<{
    id: string;
    kind: InteractionKind;
    title?: string | null;
    description?: string | null;
    prompt?: string | null;
    options: Array<{
      id: string;
      label: string;
      description?: string | null;
    }>;
    status: "pending" | "resolved" | "cancelled";
    metadata: Record<string, unknown>;
    requestedAt: string;
    resolvedAt?: string | null;
  }>;
  updatedAt: string;
};

type CodeUiEventEnvelope = {
  seq: number;
  type: string;
  at: string;
  data: CodeUiSessionSnapshot;
};

type ControllerAttachResponse = {
  controllerToken: string;
};

const CLIENT_ID_KEY = "libra.code-ui.client-id";
const CONTROLLER_TOKEN_KEY = "libra.code-ui.controller-token";

const sessionStatusLabel: Record<SessionStatus, string> = {
  idle: "Idle",
  thinking: "Thinking",
  executing_tool: "Running Tools",
  awaiting_interaction: "Awaiting Input",
  completed: "Completed",
  error: "Error",
};

const sessionStatusTone = {
  idle: "outline",
  thinking: "secondary",
  executing_tool: "secondary",
  awaiting_interaction: "destructive",
  completed: "default",
  error: "destructive",
} as const satisfies Record<
  SessionStatus,
  "default" | "secondary" | "destructive" | "outline"
>;

function formatRelativeTime(value: string) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "";
  }

  const diffMs = date.getTime() - Date.now();
  const diffMins = Math.round(diffMs / 60000);
  if (Math.abs(diffMins) < 1) {
    return "just now";
  }
  if (Math.abs(diffMins) < 60) {
    return `${Math.abs(diffMins)}m ${diffMins < 0 ? "ago" : "from now"}`;
  }

  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
    day: "numeric",
  }).format(date);
}

function createClientId() {
  return globalThis.crypto?.randomUUID?.() ?? `client-${Date.now()}`;
}

function getOrCreateClientId() {
  const existing = window.localStorage.getItem(CLIENT_ID_KEY);
  if (existing) {
    return existing;
  }
  const next = createClientId();
  window.localStorage.setItem(CLIENT_ID_KEY, next);
  return next;
}

async function apiFetch<T>(
  path: string,
  init?: RequestInit,
  controllerToken?: string | null,
): Promise<T> {
  const headers = new Headers(init?.headers);
  headers.set("content-type", "application/json");
  if (controllerToken) {
    headers.set("x-code-controller-token", controllerToken);
  }

  const response = await fetch(path, {
    ...init,
    headers,
    cache: "no-store",
  });
  if (!response.ok) {
    const payload = (await response.json().catch(() => null)) as
      | { error?: { message?: string } }
      | null;
    throw new Error(payload?.error?.message ?? `Request failed: ${response.status}`);
  }
  return (await response.json()) as T;
}

function transcriptLabel(kind: TranscriptKind) {
  switch (kind) {
    case "user_message":
      return "Developer";
    case "assistant_message":
      return "Assistant";
    case "tool_call":
      return "Tool";
    case "plan_summary":
      return "Plan";
    case "diff":
      return "Patch";
    case "info_note":
      return "Note";
  }
}

function interactionResponseForOption(optionId: string) {
  switch (optionId) {
    case "approve":
      return { approved: true };
    case "approve_all":
      return { approved: true, applyToFuture: "accept_all" };
    case "decline":
      return { approved: false };
    case "decline_all":
      return { approved: false, applyToFuture: "decline_all" };
    default:
      return { selectedOption: optionId };
  }
}

function controllerSummary(session: CodeUiSessionSnapshot | null, clientId: string | null) {
  if (!session) {
    return "Loading session";
  }
  if (session.controller.kind === "browser" && session.controller.ownerLabel === clientId) {
    return "This browser controls the session";
  }
  if (session.controller.kind === "browser") {
    return `Browser controlled by ${session.controller.ownerLabel ?? "another client"}`;
  }
  if (session.controller.kind === "cli") {
    return session.controller.reason ?? "Terminal control is active";
  }
  if (session.controller.kind === "tui") {
    return session.controller.reason ?? "Terminal UI control is active";
  }
  return session.controller.reason ?? "No controller attached";
}

function StatusDot({ status }: { status: SessionStatus }) {
  const className = {
    idle: "bg-stone-400",
    thinking: "bg-amber-500",
    executing_tool: "bg-sky-500",
    awaiting_interaction: "bg-rose-500",
    completed: "bg-emerald-500",
    error: "bg-rose-600",
  }[status];

  return <span className={cn("size-2 rounded-full", className)} aria-hidden />;
}

function SectionCard({
  title,
  eyebrow,
  children,
}: {
  title: string;
  eyebrow?: string;
  children: ReactNode;
}) {
  return (
    <section className="rounded-[28px] border border-stone-200/80 bg-white/80 p-5 shadow-[0_20px_70px_-45px_rgba(68,44,17,0.45)] backdrop-blur">
      <div className="mb-4 flex items-center justify-between gap-3">
        <div>
          {eyebrow ? (
            <p className="mb-1 text-[0.72rem] font-semibold uppercase tracking-[0.24em] text-stone-500">
              {eyebrow}
            </p>
          ) : null}
          <h2 className="text-lg font-semibold text-stone-900">{title}</h2>
        </div>
      </div>
      {children}
    </section>
  );
}

export function CodeSessionPage() {
  const [clientId, setClientId] = useState<string | null>(null);
  const [controllerToken, setControllerToken] = useState<string | null>(null);
  const [session, setSession] = useState<CodeUiSessionSnapshot | null>(null);
  const [draft, setDraft] = useState("");
  const [pageError, setPageError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [sending, setSending] = useState(false);
  const [pendingInteractionId, setPendingInteractionId] = useState<string | null>(null);
  const [streamState, setStreamState] = useState<"connecting" | "live" | "reconnecting">(
    "connecting",
  );

  const applySnapshot = useEffectEvent((nextSession: CodeUiSessionSnapshot) => {
    startTransition(() => {
      setSession(nextSession);
      setLoading(false);
      setPageError(null);
    });
  });

  const loadSession = useEffectEvent(async () => {
    const snapshot = await apiFetch<CodeUiSessionSnapshot>("/api/code/session");
    applySnapshot(snapshot);
  });

  const attachBrowserController = useEffectEvent(async () => {
    if (!clientId) {
      return;
    }
    try {
      const response = await apiFetch<ControllerAttachResponse>(
        "/api/code/controller/attach",
        {
          method: "POST",
          body: JSON.stringify({ clientId }),
        },
      );
      window.localStorage.setItem(CONTROLLER_TOKEN_KEY, response.controllerToken);
      setControllerToken(response.controllerToken);
      await loadSession();
      setActionError(null);
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "Failed to attach controller");
    }
  });

  const detachBrowserController = useEffectEvent(async () => {
    if (!clientId || !controllerToken) {
      return;
    }
    try {
      await apiFetch<{ detached: boolean }>(
        "/api/code/controller/detach",
        {
          method: "POST",
          body: JSON.stringify({ clientId }),
          keepalive: true,
        },
        controllerToken,
      );
    } catch {
      // Best effort cleanup.
    } finally {
      window.localStorage.removeItem(CONTROLLER_TOKEN_KEY);
      setControllerToken(null);
    }
  });

  useEffect(() => {
    const nextClientId = getOrCreateClientId();
    setClientId(nextClientId);
    setControllerToken(window.localStorage.getItem(CONTROLLER_TOKEN_KEY));
  }, []);

  useEffect(() => {
    if (!clientId) {
      return;
    }
    void loadSession().catch((error: unknown) => {
      setPageError(error instanceof Error ? error.message : "Failed to load session");
      setLoading(false);
    });
  }, [clientId]);

  useEffect(() => {
    if (!clientId) {
      return;
    }

    const eventSource = new EventSource("/api/code/events");
    const handleEvent = (event: Event) => {
      const payload = JSON.parse((event as MessageEvent<string>).data) as CodeUiEventEnvelope;
      applySnapshot(payload.data);
      setStreamState("live");
    };

    setStreamState("connecting");
    eventSource.addEventListener("session_updated", handleEvent);
    eventSource.addEventListener("controller_changed", handleEvent);
    eventSource.addEventListener("status_changed", handleEvent);
    eventSource.onerror = () => {
      setStreamState("reconnecting");
    };

    return () => {
      eventSource.removeEventListener("session_updated", handleEvent);
      eventSource.removeEventListener("controller_changed", handleEvent);
      eventSource.removeEventListener("status_changed", handleEvent);
      eventSource.close();
    };
  }, [clientId]);

  useEffect(() => {
    if (!clientId) {
      return;
    }

    if (session?.controller.kind === "none" && session.capabilities.messageInput) {
      void attachBrowserController();
    }

    if (session?.controller.kind !== "browser" || session.controller.ownerLabel !== clientId) {
      return;
    }

    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") {
        void attachBrowserController();
      }
    }, 30_000);

    return () => window.clearInterval(timer);
  }, [clientId, session]);

  useEffect(() => {
    if (!clientId || !controllerToken) {
      return;
    }

    const onUnload = () => {
      void detachBrowserController();
    };
    window.addEventListener("beforeunload", onUnload);
    return () => {
      window.removeEventListener("beforeunload", onUnload);
    };
  }, [clientId, controllerToken]);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!draft.trim() || !controllerToken) {
      return;
    }
    setSending(true);
    setActionError(null);
    try {
      await apiFetch<{ accepted: boolean }>(
        "/api/code/messages",
        {
          method: "POST",
          body: JSON.stringify({ text: draft.trim() }),
        },
        controllerToken,
      );
      setDraft("");
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "Failed to send message");
    } finally {
      setSending(false);
    }
  }

  async function handleInteraction(interactionId: string, optionId: string) {
    if (!controllerToken) {
      return;
    }
    setPendingInteractionId(interactionId);
    setActionError(null);
    try {
      await apiFetch<{ accepted: boolean }>(
        `/api/code/interactions/${interactionId}`,
        {
          method: "POST",
          body: JSON.stringify(interactionResponseForOption(optionId)),
        },
        controllerToken,
      );
    } catch (error) {
      setActionError(error instanceof Error ? error.message : "Failed to respond");
    } finally {
      setPendingInteractionId(null);
    }
  }

  const isBrowserController =
    session?.controller.kind === "browser" && session.controller.ownerLabel === clientId;
  const canWrite = Boolean(session?.controller.canWrite && isBrowserController && controllerToken);
  const pendingInteractions =
    session?.interactions.filter((interaction) => interaction.status === "pending") ?? [];

  return (
    <main className="min-h-screen bg-[radial-gradient(circle_at_top_left,_rgba(251,191,36,0.16),_transparent_36%),linear-gradient(180deg,_#f8f3ea_0%,_#fdfbf7_48%,_#f1e9dd_100%)] text-stone-900">
      <div className="mx-auto flex min-h-screen max-w-[1500px] flex-col gap-6 px-4 py-6 sm:px-6 lg:px-8">
        <section className="overflow-hidden rounded-[32px] border border-stone-200/80 bg-[linear-gradient(135deg,rgba(41,37,36,0.98),rgba(68,64,60,0.92))] px-6 py-6 text-stone-50 shadow-[0_28px_90px_-50px_rgba(41,37,36,0.75)] sm:px-8">
          <div className="flex flex-col gap-6 lg:flex-row lg:items-end lg:justify-between">
            <div className="max-w-3xl">
              <p className="mb-3 text-[0.72rem] font-semibold uppercase tracking-[0.3em] text-amber-200/80">
                Libra Code UI
              </p>
              <div className="mb-4 flex flex-wrap items-center gap-3">
                <Badge
                  variant={session ? sessionStatusTone[session.status] : "outline"}
                  className="border-white/10 bg-white/10 text-white"
                >
                  <span className="inline-flex items-center gap-2">
                    <StatusDot status={session?.status ?? "idle"} />
                    {session ? sessionStatusLabel[session.status] : "Loading"}
                  </span>
                </Badge>
                <Badge variant="outline" className="border-white/15 text-white/90">
                  {session?.provider.provider ?? "provider"}{session?.provider.model ? ` / ${session.provider.model}` : ""}
                </Badge>
                <Badge variant="outline" className="border-white/15 text-white/90">
                  {streamState === "live" ? "SSE Live" : streamState === "connecting" ? "Connecting" : "Reconnecting"}
                </Badge>
              </div>
              <h1 className="max-w-3xl text-3xl font-semibold tracking-tight text-white sm:text-4xl">
                Unified browser session for `libra code`, independent of provider protocol.
              </h1>
              <p className="mt-3 max-w-2xl text-sm leading-6 text-stone-300">
                The browser consumes Libra&apos;s stable session snapshot and event stream.
                Provider-specific transport details stay behind the adapter boundary.
              </p>
            </div>

            <div className="w-full max-w-xl rounded-[28px] border border-white/10 bg-white/8 p-4 backdrop-blur">
              <div className="flex items-start justify-between gap-4">
                <div>
                  <p className="text-xs font-semibold uppercase tracking-[0.24em] text-stone-300">
                    Controller
                  </p>
                  <p className="mt-2 text-sm text-stone-100">
                    {controllerSummary(session, clientId)}
                  </p>
                  {session?.controller.leaseExpiresAt ? (
                    <p className="mt-2 text-xs text-stone-300">
                      Lease until {formatRelativeTime(session.controller.leaseExpiresAt)}
                    </p>
                  ) : null}
                </div>
                <div className="flex gap-2">
                  {!isBrowserController && session?.capabilities.messageInput ? (
                    <Button
                      variant="secondary"
                      size="sm"
                      onClick={() => void attachBrowserController()}
                    >
                      Take Control
                    </Button>
                  ) : null}
                  {isBrowserController ? (
                    <Button
                      variant="outline"
                      size="sm"
                      className="border-white/20 bg-transparent text-white hover:bg-white/10"
                      onClick={() => void detachBrowserController()}
                    >
                      Release
                    </Button>
                  ) : null}
                </div>
              </div>
            </div>
          </div>
        </section>

        {pageError ? (
          <div className="rounded-[22px] border border-rose-200 bg-rose-50 px-4 py-3 text-sm text-rose-700">
            {pageError}
          </div>
        ) : null}

        {actionError ? (
          <div className="rounded-[22px] border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
            {actionError}
          </div>
        ) : null}

        <div className="grid gap-6 xl:grid-cols-[minmax(0,1.45fr)_420px]">
          <div className="flex flex-col gap-6">
            <SectionCard title="Session Timeline" eyebrow="Transcript">
              {loading ? (
                <div className="space-y-3">
                  <div className="h-20 animate-pulse rounded-[22px] bg-stone-100" />
                  <div className="h-28 animate-pulse rounded-[22px] bg-stone-100" />
                </div>
              ) : session?.transcript.length ? (
                <div className="space-y-4">
                  {session.transcript.map((entry) => (
                    <article
                      key={entry.id}
                      className={cn(
                        "rounded-[24px] border px-4 py-4",
                        entry.kind === "assistant_message"
                          ? "border-amber-200/80 bg-amber-50/80"
                          : entry.kind === "user_message"
                            ? "border-stone-200 bg-white"
                            : entry.kind === "diff"
                              ? "border-emerald-200 bg-emerald-50/80"
                              : "border-stone-200 bg-stone-50/80",
                      )}
                    >
                      <div className="mb-3 flex flex-wrap items-center gap-2">
                        <Badge variant="outline">{transcriptLabel(entry.kind)}</Badge>
                        {entry.streaming ? (
                          <Badge variant="secondary">Streaming</Badge>
                        ) : null}
                        {entry.status ? (
                          <Badge variant="ghost">{entry.status}</Badge>
                        ) : null}
                        <span className="ml-auto text-xs text-stone-500">
                          {formatRelativeTime(entry.updatedAt)}
                        </span>
                      </div>
                      {entry.title && entry.kind !== "user_message" ? (
                        <h3 className="mb-2 text-sm font-semibold text-stone-900">
                          {entry.title}
                        </h3>
                      ) : null}
                      <pre className="whitespace-pre-wrap break-words font-mono text-sm leading-6 text-stone-800">
                        {entry.content || "No content"}
                      </pre>
                    </article>
                  ))}
                </div>
              ) : (
                <EmptyState
                  icon={Sparkles}
                  title="No transcript yet"
                  description="Once the provider starts producing messages, the browser timeline will fill from the shared Code UI snapshot."
                />
              )}
            </SectionCard>

            <SectionCard title="Developer Input" eyebrow="Control Surface">
              <form className="space-y-4" onSubmit={handleSubmit}>
                <div className="rounded-[22px] border border-stone-200 bg-stone-50/80 p-4">
                  <p className="text-sm text-stone-600">
                    {canWrite
                      ? "This browser currently owns the controller lease."
                      : "The session is read-only from this browser. Attach control to send messages or answer interactions."}
                  </p>
                </div>
                <div className="flex flex-col gap-3 sm:flex-row">
                  <Input
                    value={draft}
                    onChange={(event) => setDraft(event.target.value)}
                    placeholder="Send the next developer instruction"
                    disabled={!canWrite || sending}
                    className="h-12 rounded-[18px] border-stone-300 bg-white"
                  />
                  <Button
                    type="submit"
                    size="lg"
                    disabled={!canWrite || sending || !draft.trim()}
                    className="h-12 rounded-[18px] bg-stone-900 px-5 text-white hover:bg-stone-800"
                  >
                    {sending ? "Sending..." : "Send"}
                  </Button>
                </div>
              </form>
            </SectionCard>
          </div>

          <div className="flex flex-col gap-6">
            <SectionCard title="Session Shape" eyebrow="Capabilities">
              {session ? (
                <div className="space-y-4">
                  <InfoRow icon={TerminalSquare} label="Working Dir" value={session.workingDir} />
                  <InfoRow
                    icon={PlugZap}
                    label="Provider Mode"
                    value={session.provider.mode ?? "unknown"}
                  />
                  <div className="flex flex-wrap gap-2">
                    {Object.entries(session.capabilities).map(([key, enabled]) => (
                      <Badge
                        key={key}
                        variant={enabled ? "default" : "outline"}
                        className={enabled ? "bg-stone-900 text-white" : ""}
                      >
                        {key}
                      </Badge>
                    ))}
                  </div>
                </div>
              ) : null}
            </SectionCard>

            <SectionCard title="Pending Interactions" eyebrow="Human In The Loop">
              {pendingInteractions.length ? (
                <div className="space-y-4">
                  {pendingInteractions.map((interaction) => (
                    <article
                      key={interaction.id}
                      className="rounded-[24px] border border-rose-200 bg-rose-50/70 p-4"
                    >
                      <div className="mb-3 flex items-center gap-2">
                        <ShieldAlert className="size-4 text-rose-700" />
                        <h3 className="text-sm font-semibold text-rose-950">
                          {interaction.title ?? "Action required"}
                        </h3>
                      </div>
                      {interaction.description ? (
                        <p className="text-sm leading-6 text-rose-900">
                          {interaction.description}
                        </p>
                      ) : null}
                      {interaction.prompt ? (
                        <pre className="mt-3 whitespace-pre-wrap break-words rounded-[18px] bg-white/80 p-3 font-mono text-xs leading-5 text-stone-800">
                          {interaction.prompt}
                        </pre>
                      ) : null}
                      <div className="mt-4 flex flex-wrap gap-2">
                        {interaction.options.map((option) => (
                          <Button
                            key={option.id}
                            variant={option.id.startsWith("decline") ? "destructive" : "secondary"}
                            size="sm"
                            disabled={!canWrite || pendingInteractionId === interaction.id}
                            onClick={() => void handleInteraction(interaction.id, option.id)}
                          >
                            {option.label}
                          </Button>
                        ))}
                      </div>
                    </article>
                  ))}
                </div>
              ) : (
                <EmptyState
                  icon={CheckCircle2}
                  title="Nothing waiting"
                  description="Approval requests, structured questions, and post-plan choices will appear here."
                />
              )}
            </SectionCard>

            <SectionCard title="Plans And Tasks" eyebrow="Execution">
              <div className="space-y-4">
                {session?.plans.length ? (
                  session.plans.map((plan) => (
                    <article key={plan.id} className="rounded-[22px] border border-stone-200 bg-stone-50/80 p-4">
                      <div className="mb-2 flex items-center gap-2">
                        <PencilLine className="size-4 text-stone-600" />
                        <h3 className="text-sm font-semibold text-stone-900">
                          {plan.title ?? "Plan"}
                        </h3>
                        <Badge variant="outline" className="ml-auto">
                          {plan.status}
                        </Badge>
                      </div>
                      {plan.summary ? (
                        <p className="text-sm leading-6 text-stone-700">{plan.summary}</p>
                      ) : null}
                      {plan.steps.length ? (
                        <div className="mt-3 space-y-2">
                          {plan.steps.map((step, index) => (
                            <div key={`${plan.id}-${index}`} className="flex items-start gap-3 text-sm text-stone-700">
                              <Clock3 className="mt-1 size-3.5 text-stone-400" />
                              <span>{step.step}</span>
                            </div>
                          ))}
                        </div>
                      ) : null}
                    </article>
                  ))
                ) : (
                  <EmptyState
                    icon={Clock3}
                    title="No plan snapshot"
                    description="The plan panel activates when the provider emits plan updates."
                  />
                )}

                {session?.tasks.length ? (
                  <div className="space-y-2">
                    {session.tasks.map((task) => (
                      <div
                        key={task.id}
                        className="flex items-start justify-between gap-3 rounded-[18px] border border-stone-200 bg-white px-4 py-3"
                      >
                        <div>
                          <p className="text-sm font-medium text-stone-900">
                            {task.title ?? task.id}
                          </p>
                          {task.details ? (
                            <p className="mt-1 text-xs text-stone-600">{task.details}</p>
                          ) : null}
                        </div>
                        <Badge variant="outline">{task.status}</Badge>
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            </SectionCard>

            <SectionCard title="Tools And Patches" eyebrow="Operational View">
              <div className="space-y-4">
                {session?.toolCalls.length ? (
                  session.toolCalls.map((toolCall) => (
                    <article key={toolCall.id} className="rounded-[22px] border border-sky-200 bg-sky-50/70 p-4">
                      <div className="mb-2 flex items-center gap-2">
                        <TerminalSquare className="size-4 text-sky-700" />
                        <h3 className="text-sm font-semibold text-sky-950">
                          {toolCall.toolName}
                        </h3>
                        <Badge variant="outline" className="ml-auto border-sky-200">
                          {toolCall.status}
                        </Badge>
                      </div>
                      {toolCall.summary ? (
                        <pre className="whitespace-pre-wrap break-words font-mono text-xs leading-5 text-sky-950">
                          {toolCall.summary}
                        </pre>
                      ) : null}
                    </article>
                  ))
                ) : (
                  <EmptyState
                    icon={TerminalSquare}
                    title="No tool activity"
                    description="Tool calls surface here through the provider adapter."
                  />
                )}

                {session?.patchsets.length ? (
                  session.patchsets.map((patchset) => (
                    <article key={patchset.id} className="rounded-[22px] border border-emerald-200 bg-emerald-50/70 p-4">
                      <div className="mb-3 flex items-center gap-2">
                        <FileCode2 className="size-4 text-emerald-700" />
                        <h3 className="text-sm font-semibold text-emerald-950">
                          Patchset {patchset.id}
                        </h3>
                        <Badge variant="outline" className="ml-auto border-emerald-200">
                          {patchset.status}
                        </Badge>
                      </div>
                      <div className="space-y-2">
                        {patchset.changes.map((change) => (
                          <div
                            key={`${patchset.id}-${change.path}`}
                            className="rounded-[18px] bg-white/80 px-3 py-3"
                          >
                            <div className="flex items-center justify-between gap-3">
                              <p className="font-mono text-xs text-stone-900">{change.path}</p>
                              <Badge variant="ghost">{change.changeType}</Badge>
                            </div>
                            {change.diff ? (
                              <pre className="mt-3 max-h-52 overflow-auto whitespace-pre-wrap break-words font-mono text-[11px] leading-5 text-stone-700">
                                {change.diff}
                              </pre>
                            ) : null}
                          </div>
                        ))}
                      </div>
                    </article>
                  ))
                ) : null}
              </div>
            </SectionCard>
          </div>
        </div>
      </div>
    </main>
  );
}

function InfoRow({
  icon: Icon,
  label,
  value,
}: {
  icon: ComponentType<{ className?: string }>;
  label: string;
  value: string;
}) {
  return (
    <div className="flex items-start gap-3 rounded-[18px] border border-stone-200 bg-stone-50/80 px-4 py-3">
      <Icon className="mt-0.5 size-4 text-stone-500" />
      <div className="min-w-0">
        <p className="text-xs font-semibold uppercase tracking-[0.2em] text-stone-500">
          {label}
        </p>
        <p className="mt-1 break-words text-sm text-stone-900">{value}</p>
      </div>
    </div>
  );
}

function EmptyState({
  icon: Icon,
  title,
  description,
}: {
  icon: ComponentType<{ className?: string }>;
  title: string;
  description: string;
}) {
  return (
    <div className="rounded-[24px] border border-dashed border-stone-200 bg-stone-50/80 px-4 py-10 text-center">
      <Icon className="mx-auto size-5 text-stone-500" />
      <h3 className="mt-4 text-sm font-semibold text-stone-900">{title}</h3>
      <p className="mx-auto mt-2 max-w-sm text-sm leading-6 text-stone-600">{description}</p>
    </div>
  );
}
