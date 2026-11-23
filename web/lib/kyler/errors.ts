// Public, server-sanitized diagnostics. Never format raw provider response bodies.
export type RequestFailure = {
  code?: string;
  message: string;
  details?: {
    model?: string;
    provider?: string;
    protocol?:
      | "auto"
      | "responses"
      | "chat_completions"
      | "anthropic_messages"
      | "gemini_generate_content";
    http_status?: number;
    provider_code?: string;
    parameter?: string;
    retryable?: boolean;
    suggestion?: string;
  } | null;
};

export function requestFailureMessage(error: RequestFailure | string | null | undefined): string {
  if (typeof error === "string") return error;
  if (!error) return "The execution failed without request diagnostics.";
  const details = error.details;
  const metadata = [
    details?.model && `Model: ${details.model}`,
    details?.protocol &&
      `API: ${{ responses: "Responses", chat_completions: "Chat Completions", anthropic_messages: "Anthropic Messages", gemini_generate_content: "Gemini GenerateContent", auto: "Auto" }[details.protocol]}`,
    details?.http_status && `HTTP ${details.http_status}`,
    details?.provider_code && `Code: ${details.provider_code}`,
    details?.parameter && `Parameter: ${details.parameter}`,
  ]
    .filter(Boolean)
    .join(" · ");
  return [error.message, details?.suggestion, metadata].filter(Boolean).join("\n");
}
