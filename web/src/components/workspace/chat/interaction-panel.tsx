/**
 * Renders the active `CodeUiInteractionRequest` in the chat pane.
 *
 * Maps each of the five v1 kinds to a focused control set:
 *   - `intent_review_choice` → Confirm / Modify / Cancel buttons.
 *   - `post_plan_choice`     → Execute Plan / Modify / Cancel + metadata footer.
 *   - `approval` /
 *     `sandbox_approval`     → option list + "apply to future" toggle.
 *   - `request_user_input`   → metadata-driven question form.
 *
 * Submission goes through `useBrowserController().respond()`, which lazily
 * attaches a browser lease before posting to `/api/code/interactions/{id}`.
 */
"use client";

import { useMemo, useState, type FormEvent } from "react";

import { useBrowserController } from "@/lib/code-ui/controller";
import { useCodeUiStore } from "@/lib/code-ui/store";
import type {
  CodeUiApplyToFuture,
  CodeUiInteractionRequest,
  CodeUiInteractionResponse,
} from "@/lib/code-ui/types";
import { cn } from "@/lib/utils";

type Question = {
  id: string;
  prompt: string;
  /** Defaults to free-text. */
  kind?: "single" | "multi" | "text";
  options?: { id: string; label: string }[];
};

export function InteractionPanel() {
  const { snapshot } = useCodeUiStore();
  const { respond, status } = useBrowserController();
  const pending = useMemo(
    () =>
      snapshot?.interactions.find((i) => i.status === "pending") ?? null,
    [snapshot],
  );

  if (!pending) return null;

  const canWrite = !!snapshot?.controller.canWrite || snapshot?.controller.kind === "none";

  return (
    <div
      id="libra-interaction-panel"
      data-libra-interaction-panel
      className="mb-4 rounded-md border border-accent-line bg-accent-soft px-4 py-3 text-[12.5px] text-ink"
    >
      <div className="mb-2 flex items-center gap-2">
        <span className="mono rounded-sm border border-accent-line bg-paper px-1.5 py-px text-[10px] font-semibold uppercase tracking-[0.06em] text-accent">
          {labelForKind(pending.kind)}
        </span>
        <span className="text-[11px] text-ink-3">
          interaction · {pending.id.slice(0, 8)}
        </span>
      </div>
      {pending.title && (
        <div className="mb-1 text-[13px] font-semibold text-ink">{pending.title}</div>
      )}
      {pending.description && (
        <div className="mb-2 text-[12.5px] leading-[1.55] text-ink-2">
          {pending.description}
        </div>
      )}
      {pending.prompt && (
        <pre className="mono mb-2 whitespace-pre-wrap rounded-md border border-rule bg-paper px-2.5 py-2 text-[11.5px] text-ink">
          {pending.prompt}
        </pre>
      )}

      <PanelBody
        request={pending}
        onSubmit={(body) => respond(pending.id, body)}
        disabled={!canWrite || status.kind === "attaching"}
      />

      {status.kind === "error" && (
        <div className="mt-2 text-[11px] text-bad">
          {status.code}: {status.message}
        </div>
      )}
    </div>
  );
}

function PanelBody({
  request,
  onSubmit,
  disabled,
}: {
  request: CodeUiInteractionRequest;
  onSubmit: (body: CodeUiInteractionResponse) => Promise<void>;
  disabled: boolean;
}) {
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const sendOption = async (
    body: CodeUiInteractionResponse,
  ) => {
    setSubmitting(true);
    setSubmitError(null);
    try {
      await onSubmit(body);
    } catch (error) {
      setSubmitError(error instanceof Error ? error.message : String(error));
    } finally {
      setSubmitting(false);
    }
  };

  const isBusy = disabled || submitting;

  switch (request.kind) {
    case "intent_review_choice":
    case "post_plan_choice":
      return (
        <div className="flex flex-wrap gap-1.5">
          {request.options.map((option) => (
            <button
              key={option.id}
              type="button"
              disabled={isBusy}
              onClick={() => sendOption({ selectedOption: option.id })}
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-[12px] font-medium",
                isBusy
                  ? "border-rule bg-paper-2 text-ink-3"
                  : "border-accent-line bg-paper text-ink hover:bg-accent-soft",
              )}
              title={option.description ?? option.label}
            >
              {option.label}
            </button>
          ))}
          {submitError && (
            <span className="text-[11px] text-bad">{submitError}</span>
          )}
        </div>
      );
    case "approval":
    case "sandbox_approval":
      return (
        <ApprovalForm
          request={request}
          isBusy={isBusy}
          onSubmit={sendOption}
          submitError={submitError}
        />
      );
    case "request_user_input":
      return (
        <UserInputForm
          request={request}
          isBusy={isBusy}
          onSubmit={sendOption}
          submitError={submitError}
        />
      );
  }
}

function ApprovalForm({
  request,
  isBusy,
  onSubmit,
  submitError,
}: {
  request: CodeUiInteractionRequest;
  isBusy: boolean;
  onSubmit: (body: CodeUiInteractionResponse) => Promise<void>;
  submitError: string | null;
}) {
  const [applyTo, setApplyTo] = useState<CodeUiApplyToFuture>("no");
  return (
    <div className="flex flex-wrap items-center gap-2">
      <div className="flex flex-wrap gap-1.5">
        {request.options.map((option) => (
          <button
            key={option.id}
            type="button"
            disabled={isBusy}
            onClick={() =>
              onSubmit({
                selectedOption: option.id,
                approved: option.id.toLowerCase() !== "deny",
                applyToFuture: applyTo,
              })
            }
            className={cn(
              "inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-[12px] font-medium",
              isBusy
                ? "border-rule bg-paper-2 text-ink-3"
                : "border-accent-line bg-paper text-ink hover:bg-accent-soft",
            )}
            title={option.description ?? option.label}
          >
            {option.label}
          </button>
        ))}
      </div>
      <label className="ml-auto inline-flex items-center gap-1 text-[11px] text-ink-3">
        Apply to future:
        <select
          value={applyTo}
          onChange={(e) => setApplyTo(e.target.value as CodeUiApplyToFuture)}
          disabled={isBusy}
          className="mono rounded-sm border border-rule bg-paper px-1.5 py-px text-[11px] text-ink"
        >
          <option value="no">no</option>
          <option value="accept_all">accept_all</option>
          <option value="decline_all">decline_all</option>
        </select>
      </label>
      {submitError && (
        <span className="text-[11px] text-bad">{submitError}</span>
      )}
    </div>
  );
}

function UserInputForm(props: {
  request: CodeUiInteractionRequest;
  isBusy: boolean;
  onSubmit: (body: CodeUiInteractionResponse) => Promise<void>;
  submitError: string | null;
}) {
  // Reset answer state whenever the active interaction id changes by
  // remounting the inner form — avoids `setState` inside `useEffect`.
  return <UserInputFormInner key={props.request.id} {...props} />;
}

function UserInputFormInner({
  request,
  isBusy,
  onSubmit,
  submitError,
}: {
  request: CodeUiInteractionRequest;
  isBusy: boolean;
  onSubmit: (body: CodeUiInteractionResponse) => Promise<void>;
  submitError: string | null;
}) {
  const questions = useMemo(() => parseQuestions(request), [request]);
  const [answers, setAnswers] = useState<Record<string, string[]>>({});

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onSubmit({ answers });
  };

  return (
    <form className="flex flex-col gap-3" onSubmit={submit}>
      {questions.map((question) => (
        <div key={question.id} className="flex flex-col gap-1.5">
          <span className="text-[11.5px] font-medium text-ink-2">
            {question.prompt}
          </span>
          {question.kind === "single" && question.options ? (
            <div className="flex flex-wrap gap-1.5">
              {question.options.map((option) => {
                const selected = (answers[question.id] ?? [])[0] === option.id;
                return (
                  <button
                    key={option.id}
                    type="button"
                    disabled={isBusy}
                    onClick={() =>
                      setAnswers((prev) => ({ ...prev, [question.id]: [option.id] }))
                    }
                    className={cn(
                      "rounded-md border px-2 py-1 text-[11.5px]",
                      selected
                        ? "border-accent bg-accent-soft text-accent"
                        : "border-rule bg-paper text-ink",
                    )}
                  >
                    {option.label}
                  </button>
                );
              })}
            </div>
          ) : question.kind === "multi" && question.options ? (
            <div className="flex flex-wrap gap-1.5">
              {question.options.map((option) => {
                const set = new Set(answers[question.id] ?? []);
                const selected = set.has(option.id);
                return (
                  <button
                    key={option.id}
                    type="button"
                    disabled={isBusy}
                    onClick={() => {
                      if (selected) {
                        set.delete(option.id);
                      } else {
                        set.add(option.id);
                      }
                      setAnswers((prev) => ({
                        ...prev,
                        [question.id]: Array.from(set),
                      }));
                    }}
                    className={cn(
                      "rounded-md border px-2 py-1 text-[11.5px]",
                      selected
                        ? "border-accent bg-accent-soft text-accent"
                        : "border-rule bg-paper text-ink",
                    )}
                  >
                    {option.label}
                  </button>
                );
              })}
            </div>
          ) : (
            <input
              type="text"
              disabled={isBusy}
              value={(answers[question.id] ?? [""])[0]}
              onChange={(event) =>
                setAnswers((prev) => ({
                  ...prev,
                  [question.id]: [event.target.value],
                }))
              }
              className="mono rounded-md border border-rule bg-paper px-2 py-1 text-[11.5px] text-ink"
            />
          )}
        </div>
      ))}
      <div className="flex items-center gap-2">
        <button
          type="submit"
          disabled={isBusy}
          className={cn(
            "rounded-md border px-2.5 py-1.5 text-[12px] font-medium",
            isBusy
              ? "border-rule bg-paper-2 text-ink-3"
              : "border-accent-line bg-paper text-ink hover:bg-accent-soft",
          )}
        >
          Submit answers
        </button>
        {submitError && (
          <span className="text-[11px] text-bad">{submitError}</span>
        )}
      </div>
    </form>
  );
}

function parseQuestions(request: CodeUiInteractionRequest): Question[] {
  const metadata = request.metadata as { questions?: Question[] } | undefined;
  if (metadata?.questions && Array.isArray(metadata.questions)) {
    return metadata.questions;
  }
  // Fall back to a single free-text question with the interaction prompt.
  return [
    {
      id: "answer",
      prompt: request.prompt ?? request.title ?? "Your response",
      kind: "text",
    },
  ];
}

function labelForKind(kind: CodeUiInteractionRequest["kind"]): string {
  switch (kind) {
    case "intent_review_choice":
      return "Intent review";
    case "post_plan_choice":
      return "Plan choice";
    case "approval":
      return "Approval";
    case "sandbox_approval":
      return "Sandbox approval";
    case "request_user_input":
      return "User input";
  }
}
