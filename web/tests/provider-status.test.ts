import assert from "node:assert/strict";
import test from "node:test";
import { modelDiscovery, providerConnection } from "../lib/kyler/provider-status.ts";

test("provider status distinguishes authentication, permissions and transport failures", () => {
  assert.equal(providerConnection("authentication_failed").label, "Authentication failed");
  assert.equal(providerConnection("permission_denied").label, "Permission denied");
  assert.equal(providerConnection("network_error").label, "Network error");
  assert.equal(providerConnection("connected").label, "Connected");
  assert.equal(providerConnection("reachable").label, "Reachable · authentication unverified");
});
test("model-list failure and unverified connection remain separate", () => {
  assert.match(modelDiscovery("stale"), /saved models remain available/);
  assert.equal(providerConnection("stale").label, "Not verified");
  assert.match(modelDiscovery("disabled"), /disabled/);
});
