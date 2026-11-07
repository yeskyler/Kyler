"use client";

import { Fragment, useEffect, useRef, useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import { BookOpenIcon, PlugIcon, PlusIcon, UploadIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { api } from "@/lib/kyler/client";
import type { Connector, Skill } from "@/lib/kyler/types";

export function ResourcesPanel() {
  const [tab, setTab] = useState<"skills" | "connectors">("skills");
  return (
    <section className="flex h-full min-h-0 flex-col" aria-label="Plugins">
      <header className="border-b px-6 py-4 sm:px-10">
        <div role="tablist" aria-label="Resource type" className="flex gap-1">
          {(["skills", "connectors"] as const).map((value) => (
            <button
              key={value}
              type="button"
              role="tab"
              tabIndex={tab === value ? 0 : -1}
              onKeyDown={(event) => {
                if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
                  event.preventDefault();
                  const next =
                    event.key === "Home"
                      ? "skills"
                      : event.key === "End"
                        ? "connectors"
                        : tab === "skills"
                          ? "connectors"
                          : "skills";
                  setTab(next);
                  document.getElementById(`tab-${next}`)?.focus();
                }
              }}
              aria-selected={tab === value}
              aria-controls={`resource-${value}`}
              id={`tab-${value}`}
              onClick={() => setTab(value)}
              className={`flex items-center gap-2 rounded-lg px-4 py-2 text-sm font-medium ${tab === value ? "bg-foreground text-background" : "text-muted-foreground hover:bg-muted"}`}
            >
              {value === "skills" ? (
                <BookOpenIcon className="size-4" />
              ) : (
                <PlugIcon className="size-4" />
              )}
              {value === "skills" ? "Skills" : "Connectors"}
            </button>
          ))}
        </div>
      </header>
      <div
        className="min-h-0 flex-1 overflow-y-auto px-6 py-6 sm:px-10"
        role="tabpanel"
        id={`resource-${tab}`}
        aria-labelledby={`tab-${tab}`}
      >
        {tab === "skills" ? <Skills /> : <Connectors />}
      </div>
    </section>
  );
}

function Skills() {
  const base = "plugins/skills";
  const archiveInput = useRef<HTMLInputElement>(null);
  const [notice, setNotice] = useState("");
  const running = useAuiState((state) => state.thread.isRunning);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [review, setReview] = useState<{ info: Skill; instructions: string }>();
  const [removing, setRemoving] = useState<Skill>();
  const [removeError, setRemoveError] = useState("");
  useEffect(() => {
    let disposed = false;
    api<Skill[]>(`${base}`)
      .then((value) => {
        if (!disposed) setSkills(value);
      })
      .catch((error: Error) => {
        if (!disposed) setError(error.message);
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, []);
  async function install(file: File) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      if (!file.name.toLowerCase().endsWith(".zip"))
        throw new Error("Choose a ZIP containing one skill and its SKILL.md.");
      if (file.size > 20 * 1024 * 1024) throw new Error("Skill archive must be at most 20 MB.");
      const skill = await api<Skill>(`${base}/upload`, {
        method: "POST",
        headers: { "content-type": "application/zip" },
        body: file,
      });
      setSkills(await api<Skill[]>(base));
      setNotice(`Installed ${skill.name}. Use @${skill.name} in chat.`);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function uninstall() {
    if (!removing || removing.source !== "user") return;
    setBusy(true);
    setRemoveError("");
    setError("");
    setNotice("");
    try {
      await api(`${base}/${removing.id}`, {
        method: "DELETE",
        body: JSON.stringify({ fingerprint: removing.fingerprint }),
      });
      setSkills((items) => items.filter((skill) => skill.id !== removing.id));
      setReview((current) => (current?.info.id === removing.id ? undefined : current));
      setRemoving(undefined);
      setNotice(`Uninstalled ${removing.name}.`);
      try {
        setSkills(await api<Skill[]>(base));
      } catch {
        setError(
          "Skill uninstalled, but the list could not refresh. Reload this page to see the latest skills.",
        );
      }
    } catch (error) {
      setRemoveError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function inspect(skill: Skill) {
    setBusy(true);
    setError("");
    try {
      setReview(await api(`${base}/${skill.id}`));
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function toggle(skill: Skill) {
    setBusy(true);
    setError("");
    try {
      await api(`${base}/${skill.id}/${skill.status === "enabled" ? "disable" : "enable"}`, {
        method: "POST",
        body: JSON.stringify({ fingerprint: skill.fingerprint }),
      });
      setSkills(await api(`${base}`));
      setReview(undefined);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  const list = (items: Skill[]) =>
    items.length ? (
      <div className="grid gap-3 lg:grid-cols-2">
        {items.map((skill) => (
          <article key={skill.id} className="rounded-xl border bg-background p-4">
            <div className="flex items-start justify-between gap-3">
              <h3 className="font-medium">{skill.name}</h3>
              <span className="text-xs text-muted-foreground">
                {skill.source === "project" ? "Project · " : ""}
                {skill.status.replaceAll("_", " ")}
              </span>
            </div>
            <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
              {skill.description}
            </p>
            {skill.error && <p className="mt-2 text-xs text-destructive">{skill.error}</p>}
            <div className="mt-4 flex gap-2">
              <Button size="sm" variant="outline" disabled={busy} onClick={() => inspect(skill)}>
                View
              </Button>
              {skill.status !== "needs_approval" && (
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={busy || running || ["invalid", "shadowed"].includes(skill.status)}
                  onClick={() => toggle(skill)}
                >
                  {skill.status === "enabled" ? "Disable" : "Enable"}
                </Button>
              )}
              {skill.source === "user" && (
                <Button
                  size="sm"
                  variant="ghost"
                  className="ml-auto text-destructive hover:bg-destructive/10 hover:text-destructive"
                  disabled={busy || skill.status === "invalid"}
                  onClick={() => {
                    setRemoveError("");
                    setRemoving(skill);
                  }}
                >
                  Uninstall
                </Button>
              )}
            </div>
          </article>
        ))}
      </div>
    ) : (
      <p className="rounded-xl border border-dashed p-5 text-sm text-muted-foreground">
        No skills in this group.
      </p>
    );
  return (
    <div className="mx-auto max-w-5xl space-y-8">
      {error && !review && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {notice && !error && (
        <p role="status" className="text-sm text-muted-foreground">
          {notice}
        </p>
      )}
      <input
        ref={archiveInput}
        type="file"
        accept=".zip,application/zip"
        aria-label="Skill ZIP archive"
        className="hidden"
        onChange={(event) => {
          const file = event.currentTarget.files?.[0];
          event.currentTarget.value = "";
          if (file) void install(file);
        }}
      />
      {loading ? (
        <p role="status" className="text-sm text-muted-foreground">
          Loading skills…
        </p>
      ) : (
        <>
          <section>
            <div className="mb-3 flex items-center justify-between">
              <h2 className="text-sm font-semibold">User added</h2>
              <div className="flex items-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() => archiveInput.current?.click()}
                >
                  <UploadIcon />
                  {busy ? "Please wait…" : "Install skill"}
                </Button>
              </div>
            </div>
            {list(skills.filter((skill) => skill.source !== "builtin"))}
          </section>
          <section>
            <h2 className="mb-3 text-sm font-semibold">System</h2>
            {list(skills.filter((skill) => skill.source === "builtin"))}
          </section>
        </>
      )}
      <Dialog
        open={!!removing}
        onOpenChange={(open) => {
          if (!open && !busy) setRemoving(undefined);
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogTitle>Uninstall {removing?.name}?</DialogTitle>
          <DialogDescription>
            Remove this skill from your user library. You can install it again later.
          </DialogDescription>
          {removeError && (
            <p role="alert" className="text-sm text-destructive">
              {removeError}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button variant="outline" disabled={busy} onClick={() => setRemoving(undefined)}>
              Cancel
            </Button>
            <Button variant="destructive" disabled={busy} onClick={uninstall}>
              {busy ? "Uninstalling…" : "Uninstall"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!review}
        onOpenChange={(open) => {
          if (!open) setReview(undefined);
        }}
      >
        <DialogContent className="sm:max-w-2xl">
          <DialogTitle>{review?.info.name}</DialogTitle>
          <DialogDescription>{review?.info.description}</DialogDescription>
          <div className="space-y-1 text-xs text-muted-foreground">
            <p>
              Source: {review?.info.source} · Status: {review?.info.status.replaceAll("_", " ")}
            </p>
            <p className="break-all">{review?.info.path}</p>
            {!!review?.info.resources.length && (
              <details>
                <summary className="cursor-pointer">
                  Resources · {review.info.resources.length}
                </summary>
                <ul className="mt-2 space-y-1 font-mono">
                  {review.info.resources.map((path) => (
                    <li key={path} className="break-all">
                      {path}
                    </li>
                  ))}
                </ul>
              </details>
            )}
          </div>
          <pre className="max-h-[55dvh] overflow-y-auto whitespace-pre-wrap rounded-lg bg-muted p-4 font-mono text-xs leading-relaxed">
            {review?.instructions}
          </pre>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <Button
            disabled={
              busy || running || !review || ["invalid", "shadowed"].includes(review.info.status)
            }
            onClick={() => review && toggle(review.info)}
          >
            {review?.info.status === "enabled" ? "Disable" : "Enable this version"}
          </Button>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function Connectors() {
  const base = "plugins/connectors";
  const running = useAuiState((state) => state.thread.isRunning);
  const [connectors, setConnectors] = useState<Connector[]>([]);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [adding, setAdding] = useState(false);
  const [id, setId] = useState("");
  const [method, setMethod] = useState<"remote" | "package">("remote");
  const [archive, setArchive] = useState<File>();
  const [removing, setRemoving] = useState<Connector>();
  const [removeError, setRemoveError] = useState("");
  const [notice, setNotice] = useState("");
  const [url, setUrl] = useState("");
  const [credential, setCredential] = useState("");
  useEffect(() => {
    let disposed = false;
    async function refresh() {
      try {
        const value = await api<Connector[]>(`${base}`);
        if (!disposed) setConnectors(value);
      } catch (error) {
        if (!disposed) setError((error as Error).message);
      } finally {
        if (!disposed) setLoading(false);
      }
    }
    void refresh();
    const timer = setInterval(refresh, 3000);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, []);
  async function uninstall() {
    if (!removing?.removable) return;
    setBusy(true);
    setRemoveError("");
    setError("");
    setNotice("");
    try {
      await api(`${base}/${removing.id}`, { method: "DELETE" });
      setConnectors((items) => items.filter((connector) => connector.id !== removing.id));
      setRemoving(undefined);
      setNotice(`Uninstalled ${removing.id}.`);
      try {
        setConnectors(await api<Connector[]>(base));
      } catch {
        setError(
          "Connector uninstalled, but the list could not refresh. Reload this page to see the latest connectors.",
        );
      }
    } catch (error) {
      setRemoveError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function action(connector: Connector, action: string) {
    setBusy(true);
    setError("");
    try {
      await api(`${base}/${connector.id}/${action}`, {
        method: "POST",
        body: "{}",
      });
      setConnectors(await api(`${base}`));
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function save() {
    setBusy(true);
    setError("");
    try {
      if (method === "package") {
        if (!archive) throw new Error("Choose a ZIP connector package.");
        if (archive.size > 20 * 1024 * 1024) throw new Error("Packages must be at most 20 MB.");
        await api(`${base}/upload`, { method: "POST", body: archive });
      } else {
        await api(`${base}`, {
          method: "POST",
          body: JSON.stringify({
            id,
            transport: "streamable_http",
            url: url.trim(),
            bearer_token_env: credential.trim() || null,
          }),
        });
      }
      setConnectors(await api(`${base}`));
      setAdding(false);
      setId("");
      setUrl("");
      setArchive(undefined);
      setCredential("");
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="mx-auto max-w-5xl space-y-5">
      <div className="flex items-center justify-between gap-4">
        <div>
          <h2 className="text-sm font-semibold">User added</h2>
          <p className="mt-1 text-sm text-muted-foreground">Connect your MCP tools to Kyler.</p>
        </div>
        <Button
          size="sm"
          variant="outline"
          disabled={running}
          onClick={() => {
            setError("");
            setAdding(true);
          }}
        >
          <PlusIcon />
          Add connector
        </Button>
      </div>
      {error && !adding && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      {notice && !error && (
        <p role="status" className="text-sm text-muted-foreground">
          {notice}
        </p>
      )}
      {loading ? (
        <p role="status" className="text-sm text-muted-foreground">
          Loading connectors…
        </p>
      ) : !connectors.length ? (
        <div className="rounded-xl border border-dashed p-8 text-center">
          <PlugIcon className="mx-auto mb-3 size-6 text-muted-foreground" />
          <h3 className="font-medium">No connectors yet</h3>
          <p className="mt-1 text-sm text-muted-foreground">
            Upload a connector package or add a remote MCP connection.
          </p>
        </div>
      ) : (
        <div className="grid gap-3 lg:grid-cols-2">
          {connectors.map((connector) => (
            <article key={connector.id} className="rounded-xl border p-4">
              <div className="flex items-center justify-between gap-3">
                <h3 className="font-medium">{connector.id}</h3>
                <span
                  className={`text-xs ${connector.status === "connected" ? "text-emerald-600" : "text-muted-foreground"}`}
                >
                  {connector.status}
                </span>
              </div>
              <p className="mt-2 text-xs text-muted-foreground">
                {connector.transport === "stdio" ? "Local process" : "Streamable HTTP"} ·{" "}
                {connector.tools.length} tools
              </p>
              {connector.error && (
                <p className="mt-2 text-xs text-destructive">{connector.error}</p>
              )}
              {connector.tools.length > 0 && (
                <details className="mt-3 text-sm">
                  <summary className="cursor-pointer text-muted-foreground">Tools</summary>
                  <ul className="mt-2 space-y-2">
                    {connector.tools.map((tool) => (
                      <li key={tool.name}>
                        <span className="font-mono text-xs">{tool.name}</span>
                        <p className="text-xs text-muted-foreground">{tool.description}</p>
                      </li>
                    ))}
                  </ul>
                </details>
              )}
              <div className="mt-4 flex gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy || running || connector.status === "connecting"}
                  onClick={() =>
                    action(connector, connector.status === "connected" ? "disconnect" : "connect")
                  }
                >
                  {connector.status === "connected" ? "Disconnect" : "Connect"}
                </Button>
                {connector.status === "connected" && (
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy || running}
                    onClick={() => action(connector, "reconnect")}
                  >
                    Reconnect
                  </Button>
                )}
                {connector.removable && (
                  <Button
                    size="sm"
                    variant="ghost"
                    className="ml-auto text-destructive hover:bg-destructive/10 hover:text-destructive"
                    disabled={busy || running}
                    onClick={() => {
                      setRemoveError("");
                      setRemoving(connector);
                    }}
                  >
                    Uninstall
                  </Button>
                )}
              </div>
            </article>
          ))}
        </div>
      )}
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
            <Button variant="outline" disabled={busy} onClick={() => setRemoving(undefined)}>
              Cancel
            </Button>
            <Button variant="destructive" disabled={busy} onClick={uninstall}>
              {busy ? "Uninstalling…" : "Uninstall"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
      <Dialog open={adding} onOpenChange={setAdding}>
        <DialogContent className="sm:max-w-lg">
          <DialogTitle>Add connector</DialogTitle>
          <DialogDescription>
            Saved for your Kyler service. Connect explicitly after adding.
          </DialogDescription>
          <div
            role="group"
            aria-label="Connector source"
            className="flex gap-1 rounded-lg bg-muted p-1"
          >
            {(["remote", "package"] as const).map((value) => (
              <button
                key={value}
                type="button"
                aria-pressed={method === value}
                onClick={() => {
                  if (value !== method) {
                    setMethod(value);
                    setArchive(undefined);
                  }
                  setError("");
                }}
                className={`flex-1 rounded-md px-3 py-2 text-sm ${method === value ? "bg-background shadow-sm" : "text-muted-foreground"}`}
              >
                {value === "remote" ? "Remote connection" : "Upload package"}
              </button>
            ))}
          </div>
          {method === "package" ? (
            <Fragment key="package">
              <label className="space-y-1 text-sm">
                Connector package
                <Input
                  type="file"
                  accept=".zip,application/zip"
                  onChange={(event) => setArchive(event.target.files?.[0])}
                />
              </label>
              <p className="text-xs text-muted-foreground">
                ZIP, up to 20 MB. Include connector.json at the root with id, command (a relative
                executable path), and optional args. No installation scripts are run.
              </p>
            </Fragment>
          ) : (
            <Fragment key="remote">
              <label className="space-y-1 text-sm">
                Name
                <Input
                  value={id}
                  maxLength={24}
                  onChange={(event) => setId(event.target.value)}
                  placeholder="my_connector"
                />
              </label>
              <label className="space-y-1 text-sm">
                MCP URL
                <Input
                  value={url}
                  onChange={(event) => setUrl(event.target.value)}
                  placeholder="https://example.com/mcp"
                />
              </label>
              <label className="space-y-1 text-sm">
                Token environment variable <span className="text-muted-foreground">(optional)</span>
                <Input
                  value={credential}
                  onChange={(event) => setCredential(event.target.value)}
                  placeholder="MY_MCP_TOKEN"
                />
              </label>
              <p className="text-xs text-muted-foreground">
                For authenticated endpoints, configure this variable before starting Kyler.
              </p>
            </Fragment>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <Button
            disabled={
              busy ||
              running ||
              (method === "package"
                ? !archive
                : !/^[A-Za-z0-9_]{1,24}$/.test(id) || !/^https?:\/\//.test(url))
            }
            onClick={save}
          >
            Add connector
          </Button>
        </DialogContent>
      </Dialog>
    </div>
  );
}
