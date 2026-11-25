import type { ModelCatalog, ModelInfo } from "./types.ts";

export type ProviderModelGroup = {
  id: string;
  label: string;
  models: ModelInfo[];
  isDefault: boolean;
  selected: boolean;
};

/** Keep connection identity even when the model names or provider brands coincide. */
export function providerModelGroups(catalog?: ModelCatalog, selectedProvider?: string) {
  if (!catalog) return [];
  const providers = new Map(catalog.providers?.map((provider) => [provider.provider, provider]));
  const ids = new Set(
    catalog.providers === undefined
      ? catalog.models.map((model) => model.provider)
      : providers.keys(),
  );
  return [...ids]
    .map((id): ProviderModelGroup => {
      const models = catalog.models.filter((model) => model.provider === id);
      return {
        id,
        label: providerTypeName(providers.get(id)?.display_name, id),
        models,
        isDefault: models.some((model) => model.profile === catalog.default_profile),
        selected: id === selectedProvider,
      };
    })
    .sort((a, b) => Number(b.selected) - Number(a.selected) || a.label.localeCompare(b.label));
}

/** Older servers append the internal connection ID to the provider type label. */
export function providerTypeName(displayName: string | undefined, id: string) {
  const suffix = ` (${id})`;
  return displayName?.endsWith(suffix)
    ? displayName.slice(0, -suffix.length)
    : displayName || (id === "default" ? "Configured provider" : id);
}

/** Connection IDs remain internal, including when older configurations contain duplicates. */
export function nextProviderId(adapter: string, existing: readonly { id: string }[]) {
  const ids = new Set(existing.map((provider) => provider.id));
  let id = adapter;
  for (let suffix = 2; ids.has(id); suffix++) id = `${adapter}-${suffix}`;
  return id;
}
