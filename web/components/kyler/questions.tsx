"use client";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useAuiState } from "@assistant-ui/react";
import { CircleHelpIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { api } from "@/lib/kyler/client";
import {
  answerLabel,
  mergeQuestions,
  questionResponse,
  pendingMessageQuestion,
  type QuestionDraft,
  type QuestionRecord,
  type QuestionResponse,
} from "@/lib/kyler/questions";

const QuestionContext = createContext<{
  records: QuestionRecord[];
  session: string;
  onUpdate: (record: QuestionRecord) => void;
}>({ records: [], session: "", onUpdate: () => {} });
export function QuestionsProvider({
  records,
  session,
  onUpdate,
  children,
}: {
  records: QuestionRecord[];
  session: string;
  onUpdate: (record: QuestionRecord) => void;
  children: ReactNode;
}) {
  return (
    <QuestionContext.Provider value={{ records, session, onUpdate }}>
      {children}
    </QuestionContext.Provider>
  );
}
export function usePendingQuestion() {
  const { records } = useContext(QuestionContext);
  const execution = useAuiState((state) => state.message.metadata.custom.executionId);
  const content = useAuiState((state) => state.message.content);
  return pendingMessageQuestion(
    records,
    execution,
    content.filter((part) => part.type === "tool-call").map((part) => part.toolCallId),
  );
}
export function MessageQuestion() {
  const record = usePendingQuestion();
  const { session, onUpdate } = useContext(QuestionContext);
  return record ? (
    <QuestionCard key={record.id} record={record} session={session} onUpdate={onUpdate} />
  ) : null;
}
export function useQuestions(session: string) {
  const [records, setRecords] = useState<QuestionRecord[]>([]);
  const reload = useRef<() => void>(() => {});
  const refresh = useCallback(() => reload.current(), []);
  const update = useCallback((record: QuestionRecord) => {
    setRecords((items) => mergeQuestions(items, [record]));
  }, []);
  useEffect(() => {
    const controller = new AbortController();
    let busy = false;
    const load = async () => {
      if (busy) return;
      busy = true;
      try {
        const records = await api<QuestionRecord[]>(`sessions/${session}/questions`, {
          signal: AbortSignal.any([controller.signal, AbortSignal.timeout(5000)]),
        });
        if (!controller.signal.aborted) setRecords((items) => mergeQuestions(items, records));
      } catch {
        /* SSE, reconnect, and window focus can recover a temporary disconnect. */
      } finally {
        busy = false;
      }
    };
    void load();
    reload.current = () => {
      void load();
    };
    window.addEventListener("focus", load);
    return () => {
      controller.abort();
      reload.current = () => {};
      window.removeEventListener("focus", load);
    };
  }, [session]);
  return { records, update, refresh };
}
export function QuestionCard({
  record,
  session,
  onUpdate,
}: {
  record: QuestionRecord;
  session: string;
  onUpdate: (record: QuestionRecord) => void;
}) {
  const [drafts, setDrafts] = useState<Record<string, QuestionDraft>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const response = questionResponse(record, drafts);
  const patch = (id: string, value: Partial<QuestionDraft>) =>
    setDrafts((items) => ({
      ...items,
      [id]: {
        ...(Object.hasOwn(items, id) ? items[id] : { selected: [], text: "", custom: false }),
        ...value,
      },
    }));
  async function submit(response: QuestionResponse) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const result = await api<QuestionRecord>(
        `sessions/${session}/questions/${record.id}/response`,
        { method: "POST", body: JSON.stringify(response), signal: AbortSignal.timeout(15000) },
      );
      onUpdate(result);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section
      aria-label="Question"
      aria-busy={busy}
      className="max-h-[50dvh] w-full shrink-0 overflow-y-auto rounded-2xl border border-border bg-muted/30 p-4 text-sm shadow-sm"
    >
      <h2 className="mb-4 flex items-center gap-2 font-medium">
        <CircleHelpIcon aria-hidden className="size-4 text-muted-foreground" />
        Your input is needed
      </h2>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (response) void submit(response);
        }}
      >
        <div className="space-y-5">
          {record.questions.map((question, index) => {
            const draft = Object.hasOwn(drafts, question.id)
              ? drafts[question.id]
              : { selected: [], text: "", custom: false };
            const custom = draft.custom || question.type === "text";
            return (
              <fieldset key={question.id} disabled={busy} className="min-w-0 space-y-2">
                <legend className="mb-2 break-words font-medium leading-6">
                  {record.questions.length > 1 ? `${index + 1}. ` : ""}
                  {question.question}
                </legend>
                {question.options.map((option) => (
                  <label
                    key={option.id}
                    className="flex cursor-pointer items-start gap-2.5 rounded-lg border border-transparent px-3 py-2 hover:bg-muted has-checked:border-border has-checked:bg-background"
                  >
                    <input
                      className="mt-1 accent-current"
                      type={question.type === "multi_select" ? "checkbox" : "radio"}
                      name={`${record.id}-${question.id}`}
                      checked={
                        draft.selected.includes(option.id) &&
                        !(custom && question.type === "single_select")
                      }
                      onChange={(event) =>
                        patch(
                          question.id,
                          question.type === "multi_select"
                            ? {
                                selected: event.target.checked
                                  ? [...draft.selected, option.id]
                                  : draft.selected.filter((id) => id !== option.id),
                              }
                            : { selected: [option.id], custom: false },
                        )
                      }
                    />
                    <span className="break-words leading-6">
                      {option.label}
                      {option.recommended && (
                        <span className="ml-2 text-xs text-muted-foreground">Recommended</span>
                      )}
                    </span>
                  </label>
                ))}
                {question.type !== "text" && (
                  <label className="flex cursor-pointer items-center gap-2.5 rounded-lg px-3 py-2 hover:bg-muted">
                    <input
                      className="accent-current"
                      type={question.type === "multi_select" ? "checkbox" : "radio"}
                      name={`${record.id}-${question.id}`}
                      checked={custom}
                      onChange={(event) =>
                        patch(question.id, {
                          custom: event.target.checked,
                          ...(question.type === "single_select" ? { selected: [] } : {}),
                        })
                      }
                    />
                    <span>Custom answer</span>
                  </label>
                )}
                {custom && (
                  <textarea
                    aria-label={`Custom answer: ${question.question}`}
                    value={draft.text}
                    onChange={(event) =>
                      patch(question.id, { text: event.target.value, custom: true })
                    }
                    maxLength={4000}
                    rows={2}
                    className="w-full resize-y rounded-lg border border-input bg-background px-3 py-2 leading-6 outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                  />
                )}
              </fieldset>
            );
          })}
        </div>
        {error && (
          <p role="alert" className="mt-3 text-xs text-destructive">
            {error}
          </p>
        )}
        <div className="mt-4 flex justify-end gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => void submit({ action: "cancel" })}
          >
            Cancel
          </Button>
          <Button type="submit" size="sm" disabled={busy || !response}>
            {busy ? "Submitting…" : "Submit"}
          </Button>
        </div>
      </form>
    </section>
  );
}
export function QuestionHistory({ callId, fallback }: { callId: string; fallback: ReactNode }) {
  const execution = useAuiState((state) => state.message.metadata.custom.executionId);
  const record = useContext(QuestionContext).records.find(
    (record) => record.tool_call_id === callId && (!execution || record.execution_id === execution),
  );
  if (!record) return fallback;
  return (
    <details className="rounded-lg px-2 py-1 text-xs leading-6 text-muted-foreground">
      <summary className="cursor-pointer outline-none focus-visible:ring-2 focus-visible:ring-ring/50">
        {record.status === "pending"
          ? "Waiting for input"
          : record.status === "answered"
            ? "Question answered"
            : `Question ${record.status}`}
      </summary>
      <dl className="space-y-2 py-2">
        {record.questions.map((question) => (
          <div key={question.id}>
            <dt className="font-medium text-foreground">{question.question}</dt>
            <dd className="whitespace-pre-wrap break-words">
              {answerLabel(
                question,
                Object.hasOwn(record.answers, question.id)
                  ? record.answers[question.id]
                  : undefined,
              ) || "No answer provided."}
            </dd>
          </div>
        ))}
      </dl>
    </details>
  );
}
