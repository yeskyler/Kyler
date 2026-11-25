import assert from "node:assert/strict";
import test from "node:test";
import {
  providerModelGroups,
  nextProviderId,
  providerTypeName,
} from "../lib/kyler/provider-models.ts";
import type { ModelCatalog } from "../lib/kyler/types.ts";

const catalog: ModelCatalog = {
  default_profile: "direct-flash",
  providers: [
    { provider: "default", display_name: "DeepSeek", adapter: "deepseek", status: "stale" },
    {
      provider: "work",
      display_name: "SiliconFlow (work)",
      adapter: "siliconflow",
      status: "ready",
    },
    {
      provider: "personal",
      display_name: "DeepSeek (personal)",
      adapter: "deepseek",
      status: "disabled",
    },
    { provider: "empty", display_name: "OpenAI (empty)", status: "pending" },
  ],
  models: [
    { profile: "direct-flash", provider: "default", model: "deepseek-flash" },
    { profile: "hosted-flash", provider: "work", model: "deepseek-flash" },
    { profile: "hosted-glm", provider: "work", model: "glm" },
    { profile: "personal-flash", provider: "personal", model: "deepseek-flash" },
  ],
};

test("groups identical model names by connection and preserves the selected profile identity", () => {
  const groups = providerModelGroups(catalog, "work");
  assert.equal(groups[0].id, "work");
  assert.equal(groups[0].selected, true);
  assert.deepEqual(
    groups[0].models.map((m) => m.profile),
    ["hosted-flash", "hosted-glm"],
  );
  assert.deepEqual(
    groups.find((g) => g.id === "default")?.models.map((m) => m.profile),
    ["direct-flash"],
  );
  assert.equal(groups.find((g) => g.id === "personal")?.label, "DeepSeek");
});

test("default is a marker, while failed discovery and empty connections remain visible", () => {
  const groups = providerModelGroups(catalog);
  const direct = groups.find((g) => g.id === "default")!;
  assert.equal(direct.label, "DeepSeek");
  assert.equal(direct.isDefault, true);
  assert.equal(direct.models.length, 1);
  assert.equal(groups.find((g) => g.id === "work")?.isDefault, false);
  assert.deepEqual(groups.find((g) => g.id === "empty")?.models, []);
});

test("older catalogs keep model ownership without treating default as a provider brand", () => {
  const groups = providerModelGroups({ ...catalog, providers: undefined });
  assert.equal(groups.find((g) => g.id === "default")?.label, "Configured provider");
  assert.deepEqual(
    groups.find((g) => g.id === "work")?.models.map((m) => m.profile),
    ["hosted-flash", "hosted-glm"],
  );
  assert.deepEqual(providerModelGroups(), []);
});

test("only configured providers appear, and foreign models never enter their groups", () => {
  const models = [
    ...catalog.models,
    { profile: "foreign", provider: "not-configured", model: "deepseek-flash" },
  ];
  const groups = providerModelGroups({ ...catalog, models });
  assert.equal(
    groups.some((group) => group.id === "not-configured"),
    false,
  );
  for (const group of groups) {
    assert.ok(group.models.every((model) => model.provider === group.id));
    assert.equal(
      group.models.some((model) => model.profile === "foreign"),
      false,
    );
  }
  assert.deepEqual(providerModelGroups({ ...catalog, models, providers: [] }), []);
});

test("provider labels show only the type and internal IDs are generated without collisions", () => {
  assert.equal(providerTypeName("OpenAI (OpenAI)", "OpenAI"), "OpenAI");
  assert.equal(providerTypeName("DeepSeek (work)", "work"), "DeepSeek");
  assert.equal(providerTypeName("DeepSeek", "default"), "DeepSeek");
  assert.equal(nextProviderId("deepseek", []), "deepseek");
  assert.equal(
    nextProviderId("deepseek", [{ id: "deepseek" }, { id: "deepseek-2" }]),
    "deepseek-3",
  );
  assert.equal(nextProviderId("openai", [{ id: "default" }]), "openai");
});
