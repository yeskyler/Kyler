import { tokenCount, validTokenCount } from "./tokens.ts";

export { tokenCount } from "./tokens.ts";

export type InspectorTab = "rollout" | "memory" | "context";
export type ContextBudget = {
  model: string;
  context_tokens: number;
  sliding_window_tokens: number;
  input_limit: number;
  output_tokens: number;
  safety_tokens: number;
};
export type RequestSnapshot = {
  execution_id: string;
  step: number;
  timestamp_ms: number;
  budget: ContextBudget;
  estimated_input_tokens: number;
  provider_input_tokens: number | null;
  components: { name: string; estimated_tokens: number }[];
  retrieved_memory_keys: string[];
  visible_events: number;
  window_start: number;
};
export type ModelResponseMetrics = { model: string; output_tokens: number; elapsed_ms: number };
export function modelThroughput(metrics: ModelResponseMetrics | null | undefined): number | null {
  if (
    !metrics ||
    !validTokenCount(metrics.output_tokens) ||
    metrics.output_tokens === 0 ||
    !Number.isFinite(metrics.elapsed_ms) ||
    metrics.elapsed_ms <= 0
  )
    return null;
  const rate = (metrics.output_tokens * 1000) / metrics.elapsed_ms;
  return Number.isFinite(rate) ? rate : null;
}
export type ContextInspection = {
  latest_model_response?: ModelResponseMetrics | null;
  session_id: string;
  latest_request: RequestSnapshot | null;
  next_request_budget: ContextBudget | null;
  budget_error: string | null;
  compression_count: number;
  last_compaction: {
    timestamp_ms: number;
    before_tokens: number;
    after_tokens: number;
    window_start: number;
  } | null;
  window_start: number;
  total_events: number;
  cumulative_input_tokens: number;
  cumulative_output_tokens: number;
};
export type RolloutItem = {
  seq: number;
  timestamp_ms: number;
  execution_id: string | null;
  kind: string;
  title: string;
  status: string | null;
};
export type RolloutPage = {
  session_id: string;
  items: RolloutItem[];
  cursor: number;
  next_cursor: number | null;
};
export type RolloutDetail = { item: RolloutItem; data: Record<string, unknown> };
export type MemoryInspection = {
  session_id: string;
  workspace_root: string | null;
  scope: string;
  summary: string | null;
  summary_timestamp_ms: number | null;
  compression_count: number;
  summarized_events: number;
  retrieved_keys: string[];
  entries: {
    key: string;
    content: string;
    tags: string[];
    revision: number;
    updated_at: number;
    source_session: string;
  }[];
  activity: {
    timestamp: number;
    call_id: string;
    action: string;
    status: string;
    key: string | null;
    execution_id: string;
  }[];
};

export function contextUsage(snapshot: RequestSnapshot | null | undefined) {
  if (!snapshot) return null;
  const reported = validTokenCount(snapshot.provider_input_tokens);
  const tokens = reported ? snapshot.provider_input_tokens! : snapshot.estimated_input_tokens;
  const capacity = snapshot.budget.context_tokens;
  if (!validTokenCount(tokens) || !validTokenCount(capacity) || capacity === 0) return null;
  return {
    tokens,
    capacity,
    percent: capacity > 0 ? Math.round((tokens / capacity) * 100) : 0,
    estimated: !reported,
  };
}

/** Category weights are estimates; allocate them against the same input shown in the overview. */
export function contextComponents(snapshot: RequestSnapshot) {
  const usage = contextUsage(snapshot);
  const weights = snapshot.components.map((part) =>
    validTokenCount(part.estimated_tokens) ? part.estimated_tokens : 0,
  );
  const total = weights.reduce((sum, weight) => sum + weight, 0);
  if (!usage || !total) return [];
  const quotas = weights.map((weight) => (usage.tokens * weight) / total);
  const allocated = quotas.map(Math.floor);
  const order = quotas
    .map((quota, index) => ({ index, fraction: quota - allocated[index] }))
    .sort((a, b) => b.fraction - a.fraction || a.index - b.index);
  const remainder = usage.tokens - allocated.reduce((sum, value) => sum + value, 0);
  for (let i = 0; i < remainder; i++) allocated[order[i].index]++;
  return snapshot.components.map((part, index) => ({
    ...part,
    estimated_tokens: allocated[index],
  }));
}

/** Format trusted checkpoint metadata, never arbitrary tool arguments, content, or state. */
export function formatCheckpointTokens(data: Record<string, unknown>) {
  const value = structuredClone(data);
  const visited = new WeakSet<object>();
  const tokenFields = new Set([
    "prompt_tokens",
    "completion_tokens",
    "total_tokens",
    "context_tokens",
    "sliding_window_tokens",
    "input_limit",
    "output_tokens",
    "safety_tokens",
    "estimated_input_tokens",
    "provider_input_tokens",
    "estimated_tokens",
    "before_tokens",
    "after_tokens",
    "last_input_tokens",
    "last_provider_input_tokens",
  ]);
  function format(metadata: unknown) {
    if (!metadata || typeof metadata !== "object") return;
    if (visited.has(metadata)) return;
    visited.add(metadata);
    if (Array.isArray(metadata)) {
      metadata.forEach(format);
      return;
    }
    const fields = metadata as Record<string, unknown>;
    for (const [key, entry] of Object.entries(fields)) {
      if (tokenFields.has(key)) fields[key] = tokenCount(entry as number | null | undefined);
      else if (key !== "state" && key !== "arguments" && key !== "content") format(entry);
    }
  }
  const payload = value.payload as Record<string, unknown> | undefined;
  if (!payload) return value;
  if (value.type === "usage_updated") {
    format(payload.usage);
    format(payload.subagent_usage);
  } else if (
    ["execution_started", "execution_updated", "execution_finished", "execution_archived"].includes(
      String(value.type),
    )
  ) {
    const execution = payload.execution as Record<string, unknown> | undefined;
    format(execution?.usage);
    format(execution?.subagent_usage);
  } else if (value.type === "context_updated" || value.type === "context_compacted") {
    format(payload.memory);
  } else if (value.type === "subagent_updated") {
    format((payload.task as Record<string, unknown> | undefined)?.usage);
  }
  return value;
}
export function mergeRollout(previous: RolloutItem[], incoming: RolloutItem[]) {
  return [...new Map([...previous, ...incoming].map((item) => [item.seq, item])).values()].sort(
    (a, b) => a.seq - b.seq,
  );
}
export function inspectionTime(value: number | null | undefined) {
  return value == null ? "Unavailable" : new Date(value).toLocaleString();
}

export type InspectionAnalytics = {
  session_id: string;
  records: number;
  execution_count: number;
  tool_calls: number;
  failed_tool_calls: number;
  last_activity_ms: number | null;
  context_request_count: number;
  context_requests: {
    execution_id: string;
    step: number;
    timestamp_ms: number;
    model: string;
    capacity: number;
    input_tokens: number;
    estimated: boolean;
  }[];
  compactions: NonNullable<ContextInspection["last_compaction"]>[];
  tools: { name: string; calls: number; succeeded: number; failed: number; pending: number }[];
  activity: {
    timestamp_ms: number;
    messages: number;
    tools: number;
    compactions: number;
    subagents: number;
  }[];
  executions: {
    execution_id: string;
    status: string;
    started_at: number;
    finished_at: number | null;
    steps: number;
  }[];
};
export function memoryTagCounts(entries: MemoryInspection["entries"]) {
  const tags = new Map<string, number>();
  for (const entry of entries)
    for (const tag of new Set(entry.tags)) tags.set(tag, (tags.get(tag) ?? 0) + 1);
  return [...tags]
    .map(([name, count]) => ({ name, count }))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name))
    .slice(0, 8);
}
export function memoryActivityBuckets(activity: MemoryInspection["activity"]) {
  const buckets = new Map<
    number,
    { timestamp: number; searches: number; writes: number; deletes: number; other: number }
  >();
  for (const event of activity) {
    const timestamp = Math.floor(event.timestamp / 3600) * 3600000;
    const bucket = buckets.get(timestamp) ?? {
      timestamp,
      searches: 0,
      writes: 0,
      deletes: 0,
      other: 0,
    };
    if (event.action === "search") bucket.searches++;
    else if (["save", "put"].includes(event.action)) bucket.writes++;
    else if (event.action === "delete") bucket.deletes++;
    else bucket.other++;
    buckets.set(timestamp, bucket);
  }
  return [...buckets.values()].sort((a, b) => a.timestamp - b.timestamp);
}
