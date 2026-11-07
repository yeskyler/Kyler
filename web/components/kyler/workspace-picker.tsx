"use client";

import { useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import { ChevronDownIcon, FolderIcon, FolderPlusIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { useChatSettings } from "./chat-settings-context";
import { api } from "@/lib/kyler/client";
import { workspaceName } from "@/lib/kyler/sessions";
import type { Workspace } from "@/lib/kyler/types";

export function WorkspacePicker({
  iconOnly = false,
  onCreated,
}: {
  iconOnly?: boolean;
  onCreated?: () => void;
}) {
  const { session, createSession } = useChatSettings();
  const running = useAuiState((state) => state.thread.isRunning);
  const [open, setOpen] = useState(false);
  const [directory, setDirectory] = useState("");
  const [title, setTitle] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  async function browse(openAfterSelection = false) {
    setBrowsing(true);
    setError("");
    try {
      const response = await fetch("/api/workspaces/pick", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: "{}",
      });
      const value = await response.json();
      if (!response.ok) throw new Error(value.message ?? "Cannot choose a directory.");
      if (!value.cancelled) {
        setDirectory(value.path);
        setTitle(value.name);
        if (openAfterSelection) setOpen(true);
      }
    } catch (error) {
      setError((error as Error).message);
      if (openAfterSelection) setOpen(true);
    } finally {
      setBrowsing(false);
    }
  }
  async function choose() {
    setBusy(true);
    setError("");
    try {
      const workspace = await api<Workspace>("workspaces", {
        method: "POST",
        body: JSON.stringify({ path: directory, title: title.trim() }),
      });
      if (!iconOnly) await createSession(workspace.path);
      onCreated?.();
      setOpen(false);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      open={open}
      onOpenChange={(value) => {
        if (!busy && !browsing) {
          if (value) {
            setDirectory("");
            setTitle("");
            setError("");
            if (iconOnly) void browse(true);
            else setOpen(true);
          } else setOpen(false);
        }
      }}
    >
      <DialogTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size={iconOnly ? "icon-xs" : "xs"}
            className={
              iconOnly
                ? "text-sidebar-foreground/55"
                : "min-w-0 max-w-[40%] gap-1.5 rounded-md text-muted-foreground"
            }
            disabled={busy || running || browsing}
            aria-label={iconOnly ? "Add workspace" : "Choose workspace"}
            title={iconOnly ? "Add workspace" : (session.workspace_root ?? "Choose workspace")}
          />
        }
      >
        {iconOnly ? (
          <FolderPlusIcon className="size-4" />
        ) : (
          <>
            <FolderIcon className="size-3.5" />
            <span className="truncate">
              {session.workspace_root ? workspaceName(session.workspace_root) : "Choose workspace"}
            </span>
            <ChevronDownIcon className="size-3" />
          </>
        )}
      </DialogTrigger>
      <DialogContent className="sm:max-w-lg">
        <DialogTitle>{iconOnly ? "Add workspace" : "Choose workspace"}</DialogTitle>
        <DialogDescription>
          Choose a local directory and give the project a title. Chats are saved only after Kyler
          replies.
        </DialogDescription>
        <label className="space-y-2 text-sm" htmlFor="workspace-directory">
          Local directory
        </label>
        <div className="flex gap-2">
          <Input
            id="workspace-directory"
            value={directory}
            readOnly
            placeholder="Select a project folder"
            className="min-w-0 flex-1"
            title={directory}
          />
          <Button
            type="button"
            variant="outline"
            disabled={busy || running || browsing}
            onClick={() => void browse()}
          >
            <FolderIcon />
            {browsing ? "Choosing…" : "Browse…"}
          </Button>
        </div>
        <label className="space-y-2 text-sm" htmlFor="workspace-title">
          Project title
          <Input
            id="workspace-title"
            value={title}
            maxLength={160}
            disabled={busy || running || browsing || !directory}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="My project"
            autoComplete="off"
          />
        </label>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        <Button
          type="button"
          disabled={busy || running || browsing || !directory || !title.trim()}
          onClick={choose}
        >
          {busy ? "Saving…" : iconOnly ? "Add workspace" : "Open workspace"}
        </Button>
      </DialogContent>
    </Dialog>
  );
}
