"use client";

import { useState } from "react";
import { ArrowLeftIcon, ChevronRightIcon, MessagesSquareIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { api } from "@/lib/kyler/client";
import type { Workspace } from "@/lib/kyler/types";

type Channel = {
  id: string;
  name: string;
  enabled: boolean;
  fields: { key: string; label: string; required: boolean }[];
};

type ChannelStatus = {
  enabled: boolean;
  bindings: { chat_id: string; workspace: string }[];
};

export function ChannelsDialog({
  workspaces,
  defaultWorkspace,
}: {
  workspaces: Workspace[];
  defaultWorkspace?: string;
}) {
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<ChannelStatus>();
  const [channels, setChannels] = useState<Channel[]>([]);
  const [provider, setProvider] = useState<Channel>();
  const [workspace, setWorkspace] = useState("");
  const [pairCommand, setPairCommand] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const choices = new Map(defaultWorkspace ? [[defaultWorkspace, "Default workspace"]] : []);
  for (const item of workspaces) {
    if (!choices.has(item.path)) choices.set(item.path, item.title);
  }
  const selected = choices.has(workspace) ? workspace : (choices.keys().next().value ?? "");

  async function perform(action: () => Promise<void>) {
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (error) {
      setError(error instanceof Error ? error.message : "Channel request failed.");
    } finally {
      setBusy(false);
    }
  }

  async function refresh(channel: Channel) {
    setStatus(await api<ChannelStatus>(`channels/${channel.id}`));
  }

  function openChannel(channel: Channel) {
    setProvider(channel);
    setStatus(undefined);
    setPairCommand("");
    void perform(() => refresh(channel));
  }

  return (
    <>
      <Button
        variant="ghost"
        disabled={busy}
        className="h-11 w-full justify-start gap-3 rounded-lg px-3 text-sm font-medium hover:bg-sidebar-accent/60"
        onClick={() => {
          setProvider(undefined);
          setStatus(undefined);
          setPairCommand("");
          setOpen(true);
          void perform(async () => {
            const result = await api<{ channels: Channel[] }>("channels");
            setChannels(result.channels);
          });
        }}
      >
        <MessagesSquareIcon className="size-4" />
        Channels
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
          <div className="min-w-0 space-y-2 pr-7">
            {provider && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="-ml-2"
                disabled={busy}
                onClick={() => {
                  setProvider(undefined);
                  setError("");
                }}
              >
                <ArrowLeftIcon />
                All channels
              </Button>
            )}
            <DialogTitle>{provider ? provider.name : "Channels"}</DialogTitle>
            <DialogDescription>
              {provider
                ? "View connection settings and pair a chat with a workspace."
                : "Choose a channel to view its setup and connected chats."}
            </DialogDescription>
          </div>
          {!provider && (
            <div className="grid gap-2 sm:grid-cols-2">
              {channels.map((channel) => (
                <button
                  key={channel.id}
                  type="button"
                  disabled={busy}
                  onClick={() => openChannel(channel)}
                  className="flex min-w-0 items-center gap-3 rounded-lg border px-3 py-2.5 text-left hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-60"
                >
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{channel.name}</p>
                    <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
                      <span
                        aria-hidden
                        className={`size-1.5 rounded-full ${channel.enabled ? "bg-emerald-500" : "bg-muted-foreground/45"}`}
                      />
                      {channel.enabled ? "Enabled" : "Not configured"}
                    </p>
                  </div>
                  <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground" />
                </button>
              ))}
            </div>
          )}
          {!status && busy && (
            <p role="status" className="text-sm text-muted-foreground">
              {provider ? `Loading ${provider.name}…` : "Loading channels…"}
            </p>
          )}
          {provider && (
            <section className="space-y-2 rounded-lg border p-4 text-sm">
              <h3 className="font-medium">Connection settings</h3>
              <p className="text-muted-foreground">
                {status?.enabled
                  ? "This channel is enabled. To edit its settings, run "
                  : "To enable this channel, run "}
                <code>kyler configure</code> on the server and open Channels.
              </p>
              <ul className="grid gap-1 text-muted-foreground sm:grid-cols-2">
                {provider.fields.map((field) => (
                  <li key={field.key}>
                    {field.label}
                    {!field.required && " (optional)"}
                  </li>
                ))}
              </ul>
            </section>
          )}
          {provider && status?.enabled && (
            <>
              <div className="space-y-2">
                <label htmlFor="channel-workspace" className="text-sm font-medium">
                  Workspace
                </label>
                <select
                  id="channel-workspace"
                  className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
                  value={selected}
                  disabled={busy || !selected}
                  onChange={(event) => {
                    setWorkspace(event.target.value);
                    setPairCommand("");
                  }}
                >
                  {!selected && <option value="">No workspaces available</option>}
                  {[...choices].map(([path, title]) => (
                    <option key={path} value={path}>
                      {title}
                    </option>
                  ))}
                </select>
                <Button
                  disabled={busy || !selected}
                  onClick={() =>
                    void perform(async () => {
                      setPairCommand("");
                      const { code } = await api<{ code: string }>(`channels/${provider.id}/pair`, {
                        method: "POST",
                        body: JSON.stringify({ workspace: selected }),
                      });
                      setPairCommand(`${provider.id === "telegram" ? "/" : "!"}pair ${code}`);
                    })
                  }
                >
                  Generate pairing code
                </Button>
              </div>
              {pairCommand && (
                <div className="space-y-1 rounded-md bg-muted p-3 text-sm">
                  <p>
                    {provider.id === "discord"
                      ? "Run /kyler in the bot’s DM and paste this into its text option within five minutes."
                      : provider.id === "email"
                        ? "Within five minutes, send a plain-text email with subject Kyler. Put your Kyler-Auth line first and this command on the next line."
                        : "Send this pairing command within five minutes."}
                  </p>
                  <code className="block break-all select-all">{pairCommand}</code>
                </div>
              )}
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <h3 className="text-sm font-medium">Connected chats</h3>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={busy}
                    onClick={() => void perform(() => refresh(provider))}
                  >
                    Refresh
                  </Button>
                </div>
                {status.bindings.length === 0 && (
                  <p className="text-sm text-muted-foreground">No connected chats.</p>
                )}
                {status.bindings.map((binding) => (
                  <div
                    key={binding.chat_id}
                    className="flex items-center gap-3 rounded-md border p-3"
                  >
                    <div className="min-w-0 flex-1">
                      <p className="text-sm">{binding.chat_id}</p>
                      <p
                        className="truncate text-xs text-muted-foreground"
                        title={binding.workspace}
                      >
                        {binding.workspace}
                      </p>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      onClick={() =>
                        void perform(async () => {
                          await api(
                            `channels/${provider.id}/${encodeURIComponent(binding.chat_id)}`,
                            {
                              method: "DELETE",
                            },
                          );
                          await refresh(provider);
                        })
                      }
                    >
                      Disconnect
                    </Button>
                  </div>
                ))}
              </div>
            </>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}
