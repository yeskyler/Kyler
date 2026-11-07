export type QuestionAnswer = { selected: string[]; text: string };
export type Question = {
  id: string;
  question: string;
  type: "single_select" | "multi_select" | "text";
  options: { id: string; label: string; recommended: boolean }[];
};
export type QuestionRecord = {
  id: string;
  session_id: string;
  execution_id: string;
  tool_call_id: string;
  questions: Question[];
  status: "pending" | "answered" | "dismissed" | "cancelled" | "interrupted";
  answers: Record<string, QuestionAnswer>;
  created_at: number;
  resolved_at: number | null;
};
export type QuestionResponse =
  | { action: "cancel" }
  | { action: "submit"; answers: Record<string, QuestionAnswer> };
export type QuestionDraft = QuestionAnswer & { custom: boolean };
export function mergeQuestions(current: QuestionRecord[], incoming: QuestionRecord[]) {
  const records = new Map(current.map((record) => [record.id, record]));
  for (const record of incoming) {
    const old = records.get(record.id);
    if (old && old.status !== "pending" && record.status === "pending") continue;
    records.set(record.id, record);
  }
  return [...records.values()].sort(
    (a, b) => a.created_at - b.created_at || a.id.localeCompare(b.id),
  );
}
export function questionResponse(
  record: QuestionRecord,
  drafts: Record<string, QuestionDraft>,
): QuestionResponse | null {
  const answers: [string, QuestionAnswer][] = [];
  for (const question of record.questions) {
    const draft = Object.hasOwn(drafts, question.id) ? drafts[question.id] : undefined;
    if (!draft) return null;
    const custom = draft.custom || question.type === "text";
    const selected =
      question.type === "text" || (custom && question.type === "single_select")
        ? []
        : draft.selected;
    const text = custom ? draft.text.trim() : "";
    if (new TextEncoder().encode(text).length > 4000 || (!selected.length && !text)) return null;
    if (question.type === "single_select" && selected.length > 1) return null;
    if (
      new Set(selected).size !== selected.length ||
      selected.some((id) => !question.options.some((option) => option.id === id))
    )
      return null;
    answers.push([question.id, { selected, text }]);
  }
  return { action: "submit", answers: Object.fromEntries(answers) };
}
export function answerLabel(question: Question, answer: QuestionAnswer | undefined) {
  if (!answer) return "";
  return [
    ...question.options
      .filter((option) => answer.selected.includes(option.id))
      .map((option) => option.label),
    ...(answer.text ? [answer.text] : []),
  ].join(", ");
}
