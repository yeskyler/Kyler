"use client";

import { useEffect, useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { ArrowLeftIcon, CheckIcon, ChevronDownIcon, PlusIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api } from "@/lib/kyler/client";

type Provider = {
  id: string;
  adapter: string;
  base_url: string;
  key_configured: boolean;
  models: string[];
  default_model: string | null;
  status: string;
};
type Preset = { adapter: string; name: string; base_url: string | null; key_required: boolean };
type Catalog = { providers: Provider[]; presets: Preset[] };
type Draft = {
  id: string;
  adapter: string;
  base_url: string;
  key: string;
  models: string[];
  make_default: boolean;
};

export function ProvidersPanel() {
  const [catalog, setCatalog] = useState<Catalog>();
  const [draft, setDraft] = useState<Draft>();
  const [editing, setEditing] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [removing, setRemoving] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    api<Catalog>("providers", { signal: controller.signal })
      .then((value) => {
        if (!controller.signal.aborted) {
          setCatalog(value);
          setError("");
        }
      })
      .catch((error: Error) => {
        if (!controller.signal.aborted) setError(error.message);
      });
    return () => controller.abort();
  }, [reload]);
  function update(value: Partial<Draft>) {
    setDirty(true);
    setDraft((current) => (current ? { ...current, ...value } : current));
    setNotice("");
  }
  async function perform(action: () => Promise<void>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await action();
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  function changed() {
    window.dispatchEvent(new Event("kyler:providers-changed"));
  }
  const previous = catalog?.providers.find((p) => p.id === editing);
  const preset = catalog?.presets.find((p) => p.adapter === draft?.adapter);
  const validConnection =
    draft &&
    /^[a-zA-Z0-9_-]{1,128}$/.test(draft.id) &&
    (editing || !catalog?.providers.some((provider) => provider.id === draft.id)) &&
    draft.base_url.trim() &&
    (!preset?.key_required || draft.key || previous?.key_configured);
  const valid = validConnection && draft?.models.some((model) => model.trim());
  const request = draft
    ? {
        adapter: draft.adapter,
        base_url: draft.base_url,
        key: draft.key || null,
        models: [...new Set(draft.models.map((m) => m.trim()).filter(Boolean))],
        make_default: draft.make_default,
      }
    : undefined;
  return (
    <div className="space-y-5">
      {draft ? (
        <>
          <Button
            variant="ghost"
            disabled={busy}
            onClick={() => {
              if (dirty && !window.confirm("Discard changes and return to providers?")) return;
              setDraft(undefined);
              setError("");
              setNotice("");
              setRemoving(false);
            }}
          >
            <ArrowLeftIcon />
            All providers
          </Button>
          <form
            className="space-y-5 rounded-xl border p-5"
            onSubmit={(event) => {
              event.preventDefault();
              if (!valid || busy) return;
              void perform(async () => {
                const next = await api<Catalog>(`providers/${encodeURIComponent(draft.id)}`, {
                  method: "PUT",
                  body: JSON.stringify(request),
                });
                setCatalog(next);
                setDraft(undefined);
                changed();
                setNotice("");
              });
            }}
          >
            <h3 className="font-medium">{editing ? "Edit provider" : "Add provider"}</h3>
            <div className="space-y-2">
              <span id="provider-type-label" className="text-sm font-medium">
                Provider type
              </span>
              <Menu.Root>
                <Menu.Trigger
                  render={
                    <Button
                      type="button"
                      variant="outline"
                      disabled={busy}
                      aria-labelledby="provider-type-label"
                      className="w-full justify-between"
                    />
                  }
                >
                  {preset?.name ?? draft.adapter}
                  <ChevronDownIcon className="size-4" />
                </Menu.Trigger>
                <Menu.Portal>
                  <Menu.Positioner sideOffset={6} className="z-50">
                    <Menu.Popup className="max-h-72 w-72 max-w-[90vw] overflow-y-auto rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg outline-none">
                      <Menu.RadioGroup
                        value={draft.adapter}
                        onValueChange={(adapter) => {
                          const preset = catalog?.presets.find((p) => p.adapter === adapter);
                          update({ adapter, base_url: preset?.base_url ?? "", key: "" });
                        }}
                      >
                        {catalog?.presets.map((p) => (
                          <Menu.RadioItem
                            key={p.adapter}
                            value={p.adapter}
                            className="flex min-h-9 items-center justify-between gap-3 rounded-lg px-3 py-2 text-sm outline-none data-[highlighted]:bg-accent"
                          >
                            {p.name}
                            <Menu.RadioItemIndicator>
                              <CheckIcon className="size-3.5" />
                            </Menu.RadioItemIndicator>
                          </Menu.RadioItem>
                        ))}
                      </Menu.RadioGroup>
                    </Menu.Popup>
                  </Menu.Positioner>
                </Menu.Portal>
              </Menu.Root>
            </div>
            <label className="block space-y-2 text-sm font-medium">
              <span>Provider name</span>
              <Input
                required
                disabled={busy || !!editing}
                value={draft.id}
                pattern="[a-zA-Z0-9_-]{1,128}"
                placeholder="e.g. openai-work"
                onChange={(e) => update({ id: e.target.value })}
              />
              <span className="block text-xs font-normal text-muted-foreground">
                Letters, numbers, underscores and hyphens. Shown in the chat model menu.
              </span>
            </label>
            <label className="block space-y-2 text-sm font-medium">
              <span>Base URL</span>
              <Input
                required
                type="url"
                disabled={busy}
                value={draft.base_url}
                onChange={(e) => update({ base_url: e.target.value })}
              />
            </label>
            <label className="block space-y-2 text-sm font-medium">
              <span>API key {preset && !preset.key_required ? "(optional)" : ""}</span>
              <Input
                type="password"
                autoComplete="new-password"
                disabled={busy}
                value={draft.key}
                placeholder={
                  previous?.key_configured ? "Configured — leave blank to keep" : "Enter API key"
                }
                onChange={(e) => update({ key: e.target.value })}
              />
            </label>
            <fieldset className="space-y-2" disabled={busy}>
              <legend className="mb-2 text-sm font-medium">Models</legend>
              {draft.models.map((model, index) => (
                <div key={index} className="flex gap-2">
                  <Input
                    aria-label={`Model ID ${index + 1}`}
                    value={model}
                    placeholder="Model ID"
                    onChange={(e) =>
                      update({
                        models: draft.models.map((m, i) => (i === index ? e.target.value : m)),
                      })
                    }
                  />
                  <Button
                    type="button"
                    variant="ghost"
                    aria-label={`Remove model ${index + 1}`}
                    disabled={draft.models.length === 1}
                    onClick={() => update({ models: draft.models.filter((_, i) => i !== index) })}
                  >
                    Remove
                  </Button>
                </div>
              ))}
              <Button
                type="button"
                variant="outline"
                onClick={() => update({ models: [...draft.models, ""] })}
              >
                <PlusIcon />
                Add model
              </Button>
              <p className="text-xs text-muted-foreground">
                The first model becomes the default when selecting this provider as the server
                default. Discovered models are also available in chat.
              </p>
            </fieldset>
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={busy}
                checked={draft.make_default}
                onChange={(e) => update({ make_default: e.target.checked })}
              />
              Use as default provider
            </label>
            <div className="flex flex-wrap justify-end gap-2">
              <Button
                type="button"
                variant="outline"
                disabled={busy || !validConnection}
                onClick={() =>
                  void perform(async () => {
                    const result = await api<{ models: string[] }>(
                      `providers/${encodeURIComponent(draft.id)}/test`,
                      { method: "POST", body: JSON.stringify(request) },
                    );
                    setNotice(`Connected. ${result.models.length} models found.`);
                    if (result.models.length)
                      setDraft((current) =>
                        current
                          ? {
                              ...current,
                              models: [
                                ...new Set([
                                  ...current.models.filter((m) => m.trim()),
                                  ...result.models,
                                ]),
                              ],
                            }
                          : current,
                      );
                  })
                }
              >
                Test connection
              </Button>
              <Button type="submit" disabled={busy || !valid}>
                {busy ? "Working…" : "Save"}
              </Button>
            </div>
          </form>
          {editing && (
            <section className="space-y-3 rounded-xl border p-5">
              <h3 className="text-sm font-medium">Delete provider</h3>
              <p className="text-sm text-muted-foreground">
                Existing conversations using these models will need a new selection. Choose another
                default provider first if this is the current default.
              </p>
              {removing ? (
                <div className="flex gap-2">
                  <Button variant="outline" disabled={busy} onClick={() => setRemoving(false)}>
                    Cancel
                  </Button>
                  <Button
                    variant="destructive"
                    disabled={busy}
                    onClick={() =>
                      void perform(async () => {
                        setCatalog(
                          await api<Catalog>(`providers/${encodeURIComponent(editing)}`, {
                            method: "DELETE",
                          }),
                        );
                        setDraft(undefined);
                        setRemoving(false);
                        changed();
                        setNotice("Provider deleted.");
                      })
                    }
                  >
                    Confirm deletion
                  </Button>
                </div>
              ) : (
                <Button variant="outline" disabled={busy} onClick={() => setRemoving(true)}>
                  Delete provider
                </Button>
              )}
            </section>
          )}
        </>
      ) : (
        <>
          <div className="flex justify-end">
            <Button
              disabled={!catalog || busy}
              onClick={() => {
                const preset = catalog?.presets[0];
                if (!preset) return;
                setDirty(false);
                setEditing(undefined);
                setError("");
                setNotice("");
                setRemoving(false);
                setDraft({
                  id: "",
                  adapter: preset.adapter,
                  base_url: preset.base_url ?? "",
                  key: "",
                  models: [""],
                  make_default: false,
                });
              }}
            >
              <PlusIcon />
              Add provider
            </Button>
          </div>
          {!catalog && !error && (
            <p role="status" className="text-sm text-muted-foreground">
              Loading providers…
            </p>
          )}
          {catalog?.providers.map((provider) => (
            <button
              type="button"
              key={provider.id}
              className="flex w-full flex-wrap items-center justify-between gap-3 rounded-xl border p-5 text-left outline-none hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring"
              onClick={() => {
                setDirty(false);
                setEditing(provider.id);
                setDraft({
                  id: provider.id,
                  adapter: provider.adapter,
                  base_url: provider.base_url,
                  key: "",
                  models: provider.models.length ? provider.models : [""],
                  make_default: false,
                });
                setError("");
                setNotice("");
                setRemoving(false);
              }}
            >
              <div className="min-w-0">
                <h3 className="font-medium">{provider.id}</h3>
                <p className="mt-1 break-all text-sm text-muted-foreground">{provider.base_url}</p>
                <p className="mt-2 text-xs text-muted-foreground">
                  {provider.models.length} models
                  {provider.default_model ? ` · Default: ${provider.default_model}` : ""}
                </p>
              </div>
              <span className="inline-flex items-center gap-2 rounded-full bg-accent px-3 py-1 text-xs">
                <span
                  aria-hidden="true"
                  className={`size-2 shrink-0 rounded-full ${provider.status === "ready" ? "bg-emerald-500" : provider.status === "stale" ? "bg-destructive" : "bg-muted-foreground/50"}`}
                />
                {provider.status === "ready"
                  ? "Connected"
                  : provider.status === "stale"
                    ? "Connection failed"
                    : "Not tested"}
              </span>
            </button>
          ))}
          {catalog && !catalog.providers.length && (
            <p className="text-sm text-muted-foreground">
              Add a provider to start choosing models in chat.
            </p>
          )}
        </>
      )}
      {notice && (
        <p role="status" className="text-sm text-muted-foreground">
          {notice}
        </p>
      )}
      {error && (
        <div role="alert" className="space-y-2 text-sm text-destructive">
          <p>{error}</p>
          {!catalog && (
            <Button variant="outline" onClick={() => setReload((value) => value + 1)}>
              Retry
            </Button>
          )}
        </div>
      )}
    </div>
  );
}
