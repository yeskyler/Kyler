import type { QuestionRecord } from "./questions.ts";
export type KylerEvent = {
  type: string;
  seq: number;
  execution_id?: string | null;
  timestamp_ms?: number;
  step?: number;
  text?: string;
  output?: string;
  output_truncated?: boolean;
  call_id?: string;
  name?: string;
  summary?: string;
  activity?: string;
  source?: { kind: "builtin" | "skill" | "mcp"; name?: string; server_id?: string };
  status?: string;
  error?: { message: string } | string;
  approval?: Approval;
  question?: QuestionRecord;
};
export type Approval = {
  id: string;
  digest: string;
  tool: string;
  preview: string;
  reason: string;
  status: string;
};

// Parse complete SSE frames, retaining UTF-8 and frame boundaries across chunks.
export async function* readEvents(response: Response): AsyncGenerator<KylerEvent> {
  if (!response.body) throw new Error("The event stream has no body.");
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    while (true) {
      const { done, value } = await reader.read();
      buffer += decoder.decode(value, { stream: !done });
      buffer = buffer.replace(/\r\n/g, "\n");
      let end: number;
      while ((end = buffer.indexOf("\n\n")) !== -1) {
        const frame = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        const data = frame
          .split("\n")
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trimStart())
          .join("\n");
        if (data) {
          const parsed = JSON.parse(data);
          if (!parsed.type)
            throw new Error(parsed.message ?? "The event stream expired. Reload the conversation.");
          yield parsed as KylerEvent;
        }
      }
      if (buffer.length > 8 * 1024 * 1024) throw new Error("The event stream frame is too large.");
      if (done) break;
    }
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
