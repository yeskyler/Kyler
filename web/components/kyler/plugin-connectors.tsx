"use client";

import { useEffect, useRef, useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import { Menu } from "@base-ui/react/menu";
import {
  PlugIcon,
  LoaderCircleIcon,
  Trash2Icon,
  RotateCwIcon,
  UnplugIcon,
  GlobeIcon,
  FileArchiveIcon,
  ChevronRightIcon,
  ArrowLeftIcon,
  CheckIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { Sheet, SheetContent, SheetTitle, SheetDescription } from "@/components/ui/sheet";
import { api } from "@/lib/kyler/client";
import { filterConnectors } from "@/lib/kyler/plugins";
import type { Connector } from "@/lib/kyler/types";
import {
  PluginActions,
  PluginEmpty,
  PluginNotice,
  PluginSkeletons,
  PluginToolbar,
  SourceBadge,
  StatusPill,
  pluginCard,
  pluginGrid,
  pluginIcon,
  pluginMenuItem,
  type PluginPanelProps,
} from "./plugin-ui";

export function PluginConnectors({ active, addSignal, onCount }: PluginPanelProps) {
  const base = "plugins/connectors";
  const running = useAuiState((state) => state.thread.isRunning);
  const [connectors, setConnectors] = useState<Connector[]>([]);
  const [query, setQuery] = useState("");
  const [source, setSource] = useState("all");
  const [status, setStatus] = useState("all");
  const [error, setError] = useState("");
  const [loadError, setLoadError] = useState("");
  const [notice, setNotice] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [busyId, setBusyId] = useState("");
  const [adding, setAdding] = useState(false);
  const [step, setStep] = useState<"choose" | "details">("choose");
  const [method, setMethod] = useState<"remote" | "package">("remote");
  const [id, setId] = useState("");
  const [url, setUrl] = useState("");
  const [credential, setCredential] = useState("");
  const [archive, setArchive] = useState<File>();
  const [removing, setRemoving] = useState<Connector>();
  const [removeError, setRemoveError] = useState("");
  const [selectedId, setSelectedId] = useState<string>();
  const [highlight, setHighlight] = useState("");
  const [reload, setReload] = useState(0);
  const seenAdd = useRef(addSignal);
  function openAdd() {
    setError("");
    setStep("choose");
    setAdding(true);
  }
  useEffect(() => {
    if (addSignal !== seenAdd.current && active) {
      setError("");
      setStep("choose");
      setAdding(true);
    }
    seenAdd.current = addSignal;
  }, [active, addSignal]);
  useEffect(() => {
    if (!loading) onCount(connectors.length);
  }, [connectors.length, loading, onCount]);
  useEffect(() => {
    if (!highlight || !active) return;
    const frame = requestAnimationFrame(() =>
      document
        .getElementById(`connector-${highlight}`)
        ?.scrollIntoView({ block: "nearest", behavior: "smooth" }),
    );
    return () => cancelAnimationFrame(frame);
  }, [highlight, active, connectors]);
  useEffect(() => {
    const controller = new AbortController();
    let refreshing = false;
    async function refresh() {
      if (refreshing || document.visibilityState === "hidden") return;
      refreshing = true;
      try {
        const items = await api<Connector[]>(base, { signal: controller.signal });
        if (!controller.signal.aborted) {
          setConnectors(items);
          setLoadError("");
        }
      } catch (failure) {
        if (!controller.signal.aborted) setLoadError((failure as Error).message);
      } finally {
        refreshing = false;
        if (!controller.signal.aborted) setLoading(false);
      }
    }
    void refresh();
    const timer = active ? setInterval(refresh, 3000) : undefined;
    if (active) document.addEventListener("visibilitychange", refresh);
    return () => {
      controller.abort();
      clearInterval(timer);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [active, reload]);
  async function uninstall() {
    if (!removing?.removable || busy || running) return;
    setBusy(true);
    setBusyId(removing.id);
    setRemoveError("");
    setError("");
    setNotice("");
    try {
      await api(`${base}/${removing.id}`, { method: "DELETE" });
      setConnectors((items) => items.filter((item) => item.id !== removing.id));
      setSelectedId((current) => (current === removing.id ? undefined : current));
      setRemoving(undefined);
      setNotice(`Uninstalled ${removing.id}.`);
    } catch (failure) {
      setRemoveError((failure as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function action(connector: Connector, action: "connect" | "disconnect" | "reconnect") {
    if (busy || running) return;
    setBusy(true);
    setBusyId(connector.id);
    setError("");
    setNotice("");
    try {
      await api(`${base}/${connector.id}/${action}`, { method: "POST", body: "{}" });
      try {
        setConnectors(await api<Connector[]>(base));
        setLoadError("");
      } catch {
        setLoadError(
          "The action was accepted, but the list could not refresh. Try refreshing the list.",
        );
      }
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function save() {
    if (busy || running) return;
    setBusy(true);
    setBusyId("add");
    setError("");
    setNotice("");
    try {
      let created: Connector;
      if (method === "package") {
        if (!archive) throw new Error("Choose a ZIP connector package.");
        if (!archive.name.toLowerCase().endsWith(".zip"))
          throw new Error("Choose a ZIP connector package.");
        if (archive.size > 20 * 1024 * 1024) throw new Error("Packages must be at most 20 MB.");
        created = await api<Connector>(`${base}/upload`, { method: "POST", body: archive });
      } else {
        created = await api<Connector>(base, {
          method: "POST",
          body: JSON.stringify({
            id,
            transport: "streamable_http",
            url: url.trim(),
            bearer_token_env: credential.trim() || null,
          }),
        });
      }
      setAdding(false);
      setId("");
      setUrl("");
      setCredential("");
      setArchive(undefined);
      setQuery("");
      setSource("all");
      setStatus("all");
      setNotice(`Added ${created.id}. Connect it when you are ready to use its tools.`);
      setHighlight(created.id);
      try {
        setConnectors(await api<Connector[]>(base));
        setLoadError("");
      } catch {
        setLoadError("Connector added, but the list could not refresh. Try refreshing the list.");
      }
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy(false);
    }
  }
  const filtered = filterConnectors(connectors, query, source, status);
  const selected = connectors.find((item) => item.id === selectedId);
  const clear = () => {
    setQuery("");
    setSource("all");
    setStatus("all");
  };
  const details = (connector: Connector) => {
    setError("");
    setSelectedId(connector.id);
  };
  return (
    <div className="space-y-5">
      <PluginToolbar
        kind="Connectors"
        query={query}
        onQuery={setQuery}
        source={source}
        onSource={setSource}
        sources={{ custom: "Custom", configured: "Configured" }}
        status={status}
        onStatus={setStatus}
        statuses={["connected", "disconnected", "connecting", "failed"]}
      />
      <PluginNotice error={!adding && !selected ? error || loadError : loadError} notice={notice} />
      {loadError && (
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={() => setReload((value) => value + 1)}
        >
          <RotateCwIcon />
          Refresh list
        </Button>
      )}
      {loading ? (
        <PluginSkeletons />
      ) : (
        <>
          <p className="text-xs text-muted-foreground">
            {filtered.length === connectors.length
              ? `${connectors.length} connectors`
              : `${filtered.length} of ${connectors.length} connectors`}
          </p>
          {!filtered.length ? (
            <PluginEmpty
              filtered={!!query || source !== "all" || status !== "all"}
              kind="connectors"
              onAdd={openAdd}
              onClear={clear}
            />
          ) : (
            <div className={pluginGrid}>
              {filtered.map((connector) => (
                <article
                  key={connector.id}
                  id={`connector-${connector.id}`}
                  className={`${pluginCard} ${highlight === connector.id ? "ring-2 ring-ring/20" : ""}`}
                >
                  <div className="mb-4 flex items-start gap-3">
                    <div className={pluginIcon}>
                      <PlugIcon className="size-5" strokeWidth={1.5} />
                    </div>
                    <div className="min-w-0 flex-1 pt-0.5">
                      <h3>
                        <button
                          type="button"
                          className="max-w-full truncate text-left text-sm font-semibold outline-none hover:underline focus-visible:underline"
                          title={connector.id}
                          onClick={() => details(connector)}
                        >
                          {connector.id}
                        </button>
                      </h3>
                      <div className="mt-1.5">
                        <SourceBadge>{connector.removable ? "Custom" : "Configured"}</SourceBadge>
                      </div>
                    </div>
                    <PluginActions label={connector.id} disabled={busy}>
                      <Menu.Item className={pluginMenuItem} onClick={() => details(connector)}>
                        View details
                      </Menu.Item>
                      {connector.status === "connected" && (
                        <>
                          <Menu.Item
                            className={pluginMenuItem}
                            disabled={busy || running}
                            onClick={() => void action(connector, "reconnect")}
                          >
                            <RotateCwIcon className="size-3.5" />
                            Reconnect
                          </Menu.Item>
                          <Menu.Item
                            className={pluginMenuItem}
                            disabled={busy || running}
                            onClick={() => void action(connector, "disconnect")}
                          >
                            <UnplugIcon className="size-3.5" />
                            Disconnect
                          </Menu.Item>
                        </>
                      )}
                      {connector.removable && (
                        <>
                          <div className="my-1 border-t" role="separator" />
                          <Menu.Item
                            className={`${pluginMenuItem} text-destructive`}
                            disabled={busy || running}
                            onClick={() => {
                              setRemoveError("");
                              setRemoving(connector);
                            }}
                          >
                            <Trash2Icon className="size-3.5" />
                            Uninstall
                          </Menu.Item>
                        </>
                      )}
                    </PluginActions>
                  </div>
                  <p className="min-h-10 text-sm leading-5 text-muted-foreground">
                    {connector.transport === "stdio"
                      ? "Tools running in your local environment."
                      : "Tools provided by a remote service."}
                  </p>
                  <p className="mb-5 mt-2 text-xs text-muted-foreground">
                    {connector.status === "connected"
                      ? `${connector.tools.length} tools available`
                      : "Connect to discover available tools"}
                  </p>
                  {connector.error && (
                    <p className="mb-4 line-clamp-2 text-xs text-destructive">{connector.error}</p>
                  )}
                  <div className="mt-auto flex flex-wrap items-center justify-between gap-2 border-t border-border/60 pt-4">
                    <StatusPill status={connector.status} />
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={
                        busy ||
                        (connector.status !== "connected" && running) ||
                        connector.status === "connecting"
                      }
                      onClick={() =>
                        connector.status === "connected"
                          ? details(connector)
                          : void action(connector, "connect")
                      }
                    >
                      {((busy && busyId === connector.id) || connector.status === "connecting") && (
                        <LoaderCircleIcon className="animate-spin" />
                      )}
                      {connector.status === "connected"
                        ? "View tools"
                        : connector.status === "connecting"
                          ? "Connecting…"
                          : "Connect"}
                    </Button>
                  </div>
                </article>
              ))}
            </div>
          )}
        </>
      )}
      <Sheet
        open={!!selected}
        onOpenChange={(open) => {
          if (!open && !busy) setSelectedId(undefined);
        }}
      >
        <SheetContent className="gap-0 data-[side=right]:w-full data-[side=right]:sm:max-w-xl">
          <div className="shrink-0 border-b p-6 pr-12">
            <div className={`${pluginIcon} mb-4`}>
              <PlugIcon className="size-5" />
            </div>
            <SheetTitle className="break-words text-xl font-semibold">{selected?.id}</SheetTitle>
            <SheetDescription className="mt-2">
              Tools available through this connector.
            </SheetDescription>
            <div className="mt-4 flex items-center gap-3">
              {selected && (
                <>
                  <SourceBadge>{selected.removable ? "Custom" : "Configured"}</SourceBadge>
                  <StatusPill status={selected.status} />
                </>
              )}
            </div>
          </div>
          <div className="min-h-0 flex-1 space-y-6 overflow-y-auto p-6">
            <div className="rounded-xl border bg-muted/20 p-4">
              <p className="text-xs text-muted-foreground">Connection type</p>
              <p className="mt-1 text-sm font-medium">
                {selected?.transport === "stdio" ? "Local process" : "Remote connection"}
              </p>
            </div>
            {selected?.error && (
              <p
                role="alert"
                className="rounded-xl border border-destructive/20 bg-destructive/5 p-4 text-sm text-destructive"
              >
                {selected.error}
              </p>
            )}
            <section>
              <h3 className="text-sm font-medium">
                Available tools{" "}
                <span className="ml-1 text-muted-foreground">({selected?.tools.length ?? 0})</span>
              </h3>
              {selected?.tools.length ? (
                <ul className="mt-4 divide-y rounded-xl border px-4">
                  {selected.tools.map((tool) => (
                    <li key={tool.name} className="py-4">
                      <p className="break-all font-mono text-xs font-medium">{tool.name}</p>
                      <p className="mt-1.5 text-sm leading-relaxed text-muted-foreground">
                        {tool.description || "No description provided."}
                      </p>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="mt-3 rounded-xl border border-dashed p-5 text-sm text-muted-foreground">
                  {selected?.status === "connected"
                    ? "This connector has no available tools."
                    : "Connect this connector to see its tools."}
                </p>
              )}
            </section>
          </div>
          <div className="shrink-0 space-y-3 border-t p-6">
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {error}
              </p>
            )}
            <div className="flex gap-2">
              <Button
                type="button"
                className="flex-1"
                variant={selected?.status === "connected" ? "outline" : "default"}
                disabled={busy || running || !selected || selected.status === "connecting"}
                onClick={() =>
                  selected &&
                  void action(selected, selected.status === "connected" ? "disconnect" : "connect")
                }
              >
                {busy && <LoaderCircleIcon className="animate-spin" />}
                {selected?.status === "connected"
                  ? "Disconnect"
                  : selected?.status === "connecting"
                    ? "Connecting…"
                    : "Connect"}
              </Button>
              {selected?.status === "connected" && (
                <Button
                  type="button"
                  variant="outline"
                  disabled={busy || running}
                  onClick={() => void action(selected, "reconnect")}
                >
                  <RotateCwIcon />
                  Reconnect
                </Button>
              )}
            </div>
          </div>
        </SheetContent>
      </Sheet>
      <Dialog
        open={!!removing}
        onOpenChange={(open) => {
          if (!open && !busy) setRemoving(undefined);
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogTitle>Uninstall {removing?.id}?</DialogTitle>
          <DialogDescription>
            Disconnect this connector in all sessions and remove it from your library.
          </DialogDescription>
          {removeError && (
            <p role="alert" className="text-sm text-destructive">
              {removeError}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => setRemoving(undefined)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={busy || running}
              onClick={() => void uninstall()}
            >
              {busy ? "Uninstalling…" : "Uninstall"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
      <Dialog
        open={adding}
        onOpenChange={(open) => {
          if (!busy) {
            setAdding(open);
            setError("");
          }
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogTitle>Add a connector</DialogTitle>
          <DialogDescription>
            {step === "choose"
              ? "Choose how to connect your tools to Kyler."
              : method === "remote"
                ? "Enter your service connection details."
                : "Upload a package for a local connector."}
          </DialogDescription>
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <span
              className={`flex size-5 items-center justify-center rounded-full ${step === "choose" ? "bg-foreground text-background" : "bg-muted"}`}
            >
              {step === "details" ? <CheckIcon className="size-3" /> : "1"}
            </span>
            Choose a source
            <span className="mx-1 h-px w-6 bg-border" />
            <span
              className={`flex size-5 items-center justify-center rounded-full ${step === "details" ? "bg-foreground text-background" : "bg-muted"}`}
            >
              2
            </span>
            Add details
          </div>
          {step === "choose" ? (
            <div className="space-y-3">
              {(["remote", "package"] as const).map((value) => (
                <button
                  key={value}
                  type="button"
                  className="flex w-full items-center gap-3 rounded-xl border p-4 text-left outline-none transition-colors hover:bg-muted/40 focus-visible:ring-2 focus-visible:ring-ring/50"
                  onClick={() => {
                    setMethod(value);
                    setArchive(undefined);
                    setError("");
                    setStep("details");
                  }}
                >
                  <div className={pluginIcon}>
                    {value === "remote" ? (
                      <GlobeIcon className="size-5" />
                    ) : (
                      <FileArchiveIcon className="size-5" />
                    )}
                  </div>
                  <span className="flex-1">
                    <span className="block text-sm font-medium">
                      {value === "remote" ? "Remote connection" : "Upload a package"}
                    </span>
                    <span className="mt-1 block text-xs leading-relaxed text-muted-foreground">
                      {value === "remote"
                        ? "Connect using a service's MCP URL."
                        : "Install a local connector from a ZIP file."}
                    </span>
                  </span>
                  <ChevronRightIcon className="size-4 text-muted-foreground" />
                </button>
              ))}
            </div>
          ) : (
            <>
              {method === "package" ? (
                <div className="space-y-3">
                  <label className="block space-y-2 text-sm font-medium">
                    <span>Connector package</span>
                    <Input
                      type="file"
                      accept=".zip,application/zip"
                      disabled={busy}
                      onChange={(event) => setArchive(event.target.files?.[0])}
                    />
                  </label>
                  <p className="text-xs leading-relaxed text-muted-foreground">
                    ZIP, up to 20 MB. Include connector.json at the root with a relative executable
                    path and optional arguments. Installation scripts are not run.
                  </p>
                </div>
              ) : (
                <div className="space-y-4">
                  <label className="block space-y-2 text-sm font-medium">
                    <span>Name</span>
                    <Input
                      value={id}
                      maxLength={24}
                      disabled={busy}
                      onChange={(event) => setId(event.target.value)}
                      placeholder="my_connector"
                    />
                  </label>
                  <label className="block space-y-2 text-sm font-medium">
                    <span>MCP URL</span>
                    <Input
                      value={url}
                      disabled={busy}
                      onChange={(event) => setUrl(event.target.value)}
                      placeholder="https://example.com/mcp"
                      type="url"
                    />
                  </label>
                  <details className="rounded-xl border p-4">
                    <summary className="cursor-pointer text-sm font-medium">
                      Authentication{" "}
                      <span className="font-normal text-muted-foreground">(optional)</span>
                    </summary>
                    <label className="mt-4 block space-y-2 text-sm">
                      <span>Token environment variable</span>
                      <Input
                        value={credential}
                        disabled={busy}
                        onChange={(event) => setCredential(event.target.value)}
                        placeholder="MY_MCP_TOKEN"
                      />
                    </label>
                    <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
                      Set this variable on your service before connecting. Enter the variable name,
                      not the token.
                    </p>
                  </details>
                </div>
              )}
              {error && (
                <p role="alert" className="text-sm text-destructive">
                  {error}
                </p>
              )}
              <p className="text-xs text-muted-foreground">
                Adding saves this connector. You can connect it when you are ready.
              </p>
              <div className="flex justify-between gap-2">
                <Button
                  type="button"
                  variant="ghost"
                  disabled={busy}
                  onClick={() => setStep("choose")}
                >
                  <ArrowLeftIcon />
                  Back
                </Button>
                <Button
                  type="button"
                  disabled={
                    busy ||
                    running ||
                    (method === "package"
                      ? !archive
                      : !/^[A-Za-z0-9_]{1,24}$/.test(id) || !/^https?:\/\//i.test(url.trim()))
                  }
                  onClick={() => void save()}
                >
                  {busy && <LoaderCircleIcon className="animate-spin" />}
                  {busy ? "Adding…" : "Add connector"}
                </Button>
              </div>
            </>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
