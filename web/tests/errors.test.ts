import test from "node:test";
import assert from "node:assert/strict";
import { requestFailureMessage } from "../lib/kyler/errors.ts";

test("request failures expose the actual protocol constraint and safe diagnostics", () => {
  const result = requestFailureMessage({
    code: "model_protocol_incompatible",
    message: "Tools and reasoning require Responses.",
    details: {
      model: "gpt-5.6-sol",
      protocol: "chat_completions",
      http_status: 400,
      provider_code: "invalid_request_error",
      parameter: "reasoning_effort",
      suggestion: "Use Responses or turn reasoning Off.",
    },
  });
  assert.match(result, /Tools and reasoning/);
  for (const value of [
    "gpt-5.6-sol",
    "Chat Completions",
    "400",
    "reasoning_effort",
    "Use Responses",
  ])
    assert.ok(result.includes(value));
});
test("legacy failure messages remain readable", () => {
  assert.equal(requestFailureMessage("Legacy failure"), "Legacy failure");
  assert.equal(
    requestFailureMessage({ message: "Authentication failed" }),
    "Authentication failed",
  );
});
