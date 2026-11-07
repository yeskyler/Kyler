"use client";

import { useEffect, useRef, useState } from "react";
import { Menu } from "@base-ui/react/menu";
import { useAui, useAuiState } from "@assistant-ui/react";
import {
  BookOpenIcon,
  BlocksIcon,
  ChevronRightIcon,
  LoaderCircleIcon,
  PaperclipIcon,
  PlugIcon,
  PlusIcon,
  XIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { useChatSettings } from "./chat-settings-context";
import { api } from "@/lib/kyler/client";
import { appendPluginReference, type PluginReference } from "@/lib/kyler/plugin-references";
import type { Connector, Skill } from "@/lib/kyler/types";

const itemClass =
  "flex min-h-9 cursor-default items-center gap-2 rounded-lg px-2.5 py-2 text-sm outline-none data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground data-[disabled]:opacity-45";
const popupClass =
  "min-w-52 max-w-[calc(100vw-2rem)] rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg outline-none";

export function ComposerMenu() {
  const aui = useAui();
  const { session } = useChatSettings();
  const running = useAuiState((state) => state.thread.isRunning);
  const accept = useAuiState((state) => state.composer.attachmentAccept);
  const [open, setOpen] = useState(false);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [connectors, setConnectors] = useState<Connector[]>([]);
  const [loading, setLoading] = useState(false);
  const [selecting, setSelecting] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const files = useRef<HTMLInputElement>(null);
  const operation = useRef<AbortController | null>(null);
  const fetching = useRef(false);
  useEffect(() => () => operation.current?.abort(), []);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(""), 4000);
    return () => clearTimeout(timer);
  }, [notice]);

  async function loadPlugins() {
    if (fetching.current) return;
    fetching.current = true;
    setLoading(true);
    setError("");
    try {
      const [skills, connectors] = await Promise.all([
        api<Skill[]>("plugins/skills"),
        api<Connector[]>("plugins/connectors"),
      ]);
      setSkills(skills);
      setConnectors(connectors);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      fetching.current = false;
      setLoading(false);
    }
  }
  function insert(reference: PluginReference) {
    aui.composer().setText(appendPluginReference(aui.composer().getState().text, reference));
    setOpen(false);
  }
  async function selectConnector(connector: Connector) {
    setSelecting(connector.id);
    setError("");
    const controller = new AbortController();
    operation.current?.abort();
    operation.current = controller;
    let timedOut = false;
    const timeout = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, 12_000);
    try {
      let current = connector;
      if (!["connected", "connecting"].includes(current.status))
        await api(`plugins/connectors/${connector.id}/connect`, {
          method: "POST",
          body: "{}",
          signal: controller.signal,
        });
      while (current.status !== "connected") {
        const next = await api<Connector[]>("plugins/connectors", { signal: controller.signal });
        setConnectors(next);
        current = next.find((item) => item.id === connector.id) ?? {
          ...connector,
          status: "failed",
          error: "Connector is unavailable.",
        };
        if (current.status === "failed")
          throw new Error(
            current.error ?? "Cannot connect to this service under the current sandbox policy.",
          );
        if (current.status !== "connected")
          await new Promise<void>((resolve, reject) => {
            const stop = () => {
              clearTimeout(timer);
              reject(new Error("Selection cancelled."));
            };
            const timer = setTimeout(() => {
              controller.signal.removeEventListener("abort", stop);
              resolve();
            }, 250);
            controller.signal.addEventListener("abort", stop, { once: true });
            if (controller.signal.aborted) stop();
          });
      }
      if (!current.tools.length) throw new Error("This connector does not expose any tools.");
      insert({ kind: "connector", name: connector.id });
    } catch (error) {
      if (timedOut)
        setError("Connection timed out. Check the connector configuration and try again.");
      else if (!controller.signal.aborted) setError((error as Error).message);
    } finally {
      clearTimeout(timeout);
      if (operation.current === controller) operation.current = null;
      setSelecting("");
    }
  }
  const pluginPopup = (children: React.ReactNode) => (
    <Menu.Portal>
      <Menu.Positioner side="right" align="start" sideOffset={5} className="z-50">
        <Menu.Popup className={`${popupClass} max-h-[min(24rem,70dvh)] w-64 overflow-y-auto`}>
          {children}
        </Menu.Popup>
      </Menu.Positioner>
    </Menu.Portal>
  );
  return (
    <div className="relative shrink-0">
      <input
        ref={files}
        type="file"
        multiple
        accept={accept || undefined}
        className="hidden"
        aria-label="Upload attachments"
        onChange={async (event) => {
          const selected = Array.from(event.target.files ?? []);
          event.target.value = "";
          try {
            for (const file of selected) await aui.composer().addAttachment(file);
          } catch (error) {
            setError((error as Error).message);
          }
        }}
      />
      <Menu.Root
        open={open}
        onOpenChange={(value) => {
          setOpen(value);
          if (value) {
            setError("");
            setNotice("");
          } else operation.current?.abort();
        }}
      >
        <Menu.Trigger
          render={
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-7 rounded-full text-muted-foreground"
              aria-label="Add to message"
              disabled={running}
            />
          }
        >
          <PlusIcon className="size-4" />
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner side="top" align="start" sideOffset={8} className="z-50">
            <Menu.Popup
              className={popupClass}
              finalFocus={() =>
                document.querySelector<HTMLTextAreaElement>(
                  'textarea[aria-label="Message input"]',
                ) ?? true
              }
            >
              <Menu.Item
                className={itemClass}
                onClick={() => {
                  if (!session.workspace_root) {
                    setNotice("Choose a workspace to upload attachments.");
                    return;
                  }
                  files.current?.click();
                }}
              >
                <PaperclipIcon className="size-4 text-muted-foreground" />
                Upload attachment
              </Menu.Item>
              <Menu.SubmenuRoot
                onOpenChange={(value) => {
                  if (value) void loadPlugins();
                }}
              >
                <Menu.SubmenuTrigger className={itemClass}>
                  <BlocksIcon className="size-4 text-muted-foreground" />
                  Plugins
                  <ChevronRightIcon className="ml-auto size-3.5 text-muted-foreground" />
                </Menu.SubmenuTrigger>
                {pluginPopup(
                  <>
                    <Menu.SubmenuRoot>
                      <Menu.SubmenuTrigger className={itemClass}>
                        <BookOpenIcon className="size-4 text-muted-foreground" />
                        Skills
                        <ChevronRightIcon className="ml-auto size-3.5 text-muted-foreground" />
                      </Menu.SubmenuTrigger>
                      {pluginPopup(
                        <>
                          {loading ? (
                            <p role="status" className="p-3 text-xs text-muted-foreground">
                              Loading skills…
                            </p>
                          ) : !skills.length ? (
                            <p className="p-3 text-xs text-muted-foreground">
                              No skills available.
                            </p>
                          ) : (
                            skills.map((skill) => (
                              <Menu.Item
                                key={skill.id}
                                className={itemClass}
                                label={skill.name}
                                disabled={skill.status !== "enabled" || !!selecting}
                                title={skill.description}
                                onClick={() => insert({ kind: "skill", name: skill.name })}
                              >
                                <span className="min-w-0">
                                  <span className="block break-all">@{skill.name}</span>
                                  <span className="mt-0.5 block text-[11px] text-muted-foreground">
                                    {skill.status === "enabled"
                                      ? skill.source === "builtin"
                                        ? "System"
                                        : "User added"
                                      : skill.status.replaceAll("_", " ")}
                                  </span>
                                </span>
                              </Menu.Item>
                            ))
                          )}
                        </>,
                      )}
                    </Menu.SubmenuRoot>
                    <Menu.SubmenuRoot>
                      <Menu.SubmenuTrigger className={itemClass}>
                        <PlugIcon className="size-4 text-muted-foreground" />
                        Connectors
                        <ChevronRightIcon className="ml-auto size-3.5 text-muted-foreground" />
                      </Menu.SubmenuTrigger>
                      {pluginPopup(
                        <>
                          {loading ? (
                            <p role="status" className="p-3 text-xs text-muted-foreground">
                              Loading connectors…
                            </p>
                          ) : !connectors.length ? (
                            <p className="p-3 text-xs text-muted-foreground">
                              No connectors configured.
                            </p>
                          ) : (
                            connectors.map((connector) => (
                              <Menu.Item
                                key={connector.id}
                                className={itemClass}
                                label={connector.id}
                                closeOnClick={false}
                                disabled={!!selecting}
                                onClick={() => void selectConnector(connector)}
                              >
                                {selecting === connector.id ? (
                                  <LoaderCircleIcon className="size-4 animate-spin" />
                                ) : (
                                  <PlugIcon className="size-4 text-muted-foreground" />
                                )}
                                <span className="min-w-0">
                                  <span className="block break-all">{connector.id}</span>
                                  <span className="mt-0.5 block text-[11px] text-muted-foreground">
                                    {connector.status}
                                  </span>
                                </span>
                              </Menu.Item>
                            ))
                          )}
                        </>,
                      )}
                    </Menu.SubmenuRoot>
                  </>,
                )}
              </Menu.SubmenuRoot>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
      {notice && (
        <div
          role="status"
          className="absolute bottom-full left-0 z-50 mb-2 w-64 rounded-lg border bg-popover p-3 text-xs text-popover-foreground shadow-md"
        >
          {notice}
        </div>
      )}
      {error && (
        <div
          role="alert"
          className="absolute bottom-full left-0 z-50 mb-2 flex w-64 gap-2 rounded-lg border bg-popover p-3 text-xs text-destructive shadow-md"
        >
          <span className="flex-1">{error}</span>
          <button type="button" aria-label="Dismiss plugin error" onClick={() => setError("")}>
            <XIcon className="size-3.5" />
          </button>
        </div>
      )}
    </div>
  );
}
