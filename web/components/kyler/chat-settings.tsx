"use client";

import { useEffect, useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import { CheckIcon, ShieldIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogTrigger,
  DialogDescription,
} from "@/components/ui/dialog";
import { Menu } from "@base-ui/react/menu";
import { api } from "@/lib/kyler/client";
import { permissionLabels, type Permissions, type PermissionMode } from "@/lib/kyler/types";

import { useChatSettings } from "./chat-settings-context";
import { ComposerPlan } from "./composer-plan";
import { SessionFilesMenu } from "./artifact-browser";
import { SessionInspectorControls } from "./session-inspector";
import { WorkspacePicker } from "./workspace-picker";
export { ChatSettingsProvider } from "./chat-settings-context";

export function ModelLabel() {
  const { initialModel } = useChatSettings();
  return (
    <span
      className="min-w-0 max-w-full shrink truncate px-3 py-2 text-left text-sm leading-5 text-muted-foreground"
      title={initialModel}
    >
      {initialModel}
    </span>
  );
}

type ThinkingLevel = "high" | "max";
type ThinkingState = {
  enabled: boolean;
  supported: boolean;
  level: ThinkingLevel;
  levels: ThinkingLevel[];
};
function normalizeThinking(value: ThinkingState): ThinkingState {
  return {
    ...value,
    level: value.level === "max" ? "max" : "high",
    levels: value.levels?.filter((level) => level === "high" || level === "max") ?? ["high", "max"],
  };
}
const thinkingLabels: Record<ThinkingLevel, string> = { high: "High", max: "Max" };

export function ThinkingControl() {
  const { session, ensureSession } = useChatSettings();
  const running = useAuiState((state) => state.thread.isRunning);
  const [thinking, setThinking] = useState<ThinkingState>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    setThinking(undefined);
    setError("");
    ensureSession()
      .then((bound) =>
        api<ThinkingState>(`sessions/${bound.session_id}/thinking`, { signal: controller.signal }),
      )
      .then((value) => {
        if (!controller.signal.aborted) setThinking(normalizeThinking(value));
      })
      .catch(() => {
        if (!controller.signal.aborted) setError("Unable to load thinking settings.");
      });
    return () => controller.abort();
  }, [session.session_id, ensureSession]);
  async function select(level: ThinkingLevel) {
    if (!thinking?.levels.includes(level) || busy || running) return;
    setOpen(false);
    setBusy(true);
    setError("");
    try {
      const bound = await ensureSession();
      setThinking(
        normalizeThinking(
          await api<ThinkingState>(`sessions/${bound.session_id}/thinking`, {
            method: "PUT",
            body: JSON.stringify({ level }),
          }),
        ),
      );
    } catch {
      setError("Unable to save thinking level. Try again.");
    } finally {
      setBusy(false);
    }
  }
  const label = thinking ? thinkingLabels[thinking.level] : "…";
  return (
    <span className="shrink-0">
      <Menu.Root open={open} onOpenChange={setOpen}>
        <Menu.Trigger
          render={
            <Button
              type="button"
              variant="ghost"
              size="xs"
              disabled={running || busy || !thinking}
              aria-label={`Thinking level: ${label}`}
              aria-busy={busy}
              title={error || "Thinking level · applies to the next turn"}
              className={`px-2 font-normal ${error ? "text-destructive" : "text-muted-foreground hover:text-foreground"}`}
            />
          }
        >
          {label}
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner side="top" align="end" sideOffset={8} className="z-50">
            <Menu.Popup
              className="min-w-36 rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg outline-none"
              aria-label="Thinking level"
            >
              {thinking && thinking.levels.length === 0 && (
                <p className="max-w-56 px-3 py-2 text-xs text-muted-foreground">
                  Thinking configuration is unavailable on this backend. Update and restart it to
                  enable these options.
                </p>
              )}
              <Menu.RadioGroup value={thinking?.level}>
                {(["high", "max"] as const).map((level) => (
                  <Menu.RadioItem
                    key={level}
                    value={level}
                    onClick={() => void select(level)}
                    disabled={busy || running || !thinking?.levels.includes(level)}
                    title={
                      !thinking?.levels.includes(level)
                        ? "This model/provider does not support this level."
                        : undefined
                    }
                    className="flex min-h-9 cursor-default items-center justify-between gap-6 rounded-lg px-3 py-2 text-sm outline-none data-[highlighted]:bg-accent data-[disabled]:opacity-40"
                  >
                    {thinkingLabels[level]}
                    <Menu.RadioItemIndicator>
                      <CheckIcon className="size-3.5" aria-hidden />
                    </Menu.RadioItemIndicator>
                  </Menu.RadioItem>
                ))}
              </Menu.RadioGroup>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
      {error && (
        <span role="alert" className="sr-only">
          {error}
        </span>
      )}
    </span>
  );
}

export function ComposerSettings() {
  const { session, ensureSession } = useChatSettings();
  const running = useAuiState((state) => state.thread.isRunning);
  const [permissions, setPermissions] = useState<Permissions>();
  const [mode, setMode] = useState<PermissionMode>("workspace_access");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [permissionOpen, setPermissionOpen] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  useEffect(() => {
    if (!permissionOpen) return;
    let disposed = false;
    ensureSession()
      .then((bound) => api<Permissions>(`sessions/${bound.session_id}/permissions`))
      .then((value) => {
        if (!disposed) {
          setPermissions(value);
          setMode(value.selected.mode);
        }
      })
      .catch((error: Error) => {
        if (!disposed) setError(error.message);
      });
    return () => {
      disposed = true;
    };
  }, [session.session_id, permissionOpen, ensureSession]);
  async function savePermissions() {
    setBusy(true);
    setError("");
    try {
      const bound = await ensureSession();
      setPermissions(
        await api<Permissions>(`sessions/${bound.session_id}/permissions`, {
          method: "PUT",
          body: JSON.stringify({ mode, confirm_full_access: mode === "full_access" && confirmed }),
        }),
      );
      setPermissionOpen(false);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="flex w-full min-w-0 flex-wrap items-center gap-1 px-1 text-xs text-muted-foreground">
      <WorkspacePicker />
      <span className="text-border" aria-hidden>
        ·
      </span>
      <Dialog
        open={permissionOpen}
        onOpenChange={(open) => {
          setError("");
          setPermissionOpen(open);
          setConfirmed(false);
          setMode(permissions?.selected.mode ?? "workspace_access");
        }}
      >
        <DialogTrigger
          render={
            <Button
              type="button"
              variant="ghost"
              size="xs"
              title={
                permissions ? permissionLabels[permissions.selected.mode] : "Sandbox permissions"
              }
              aria-label={
                permissions ? permissionLabels[permissions.selected.mode] : "Sandbox permissions"
              }
              disabled={!session.workspace_root}
            />
          }
        >
          <ShieldIcon />
          <span className="hidden sm:inline">
            {permissions ? permissionLabels[permissions.selected.mode] : "Permissions"}
          </span>
        </DialogTrigger>
        <DialogContent>
          <DialogTitle>Sandbox permissions</DialogTitle>
          <DialogDescription>
            Applies to the next turn. The server enforces the available access levels.
          </DialogDescription>
          <div role="radiogroup" aria-label="Access preset" className="space-y-2">
            {permissions?.options.map((option) => (
              <label
                key={option.mode}
                className={`flex cursor-pointer gap-3 rounded-lg border p-3 ${!option.available ? "opacity-50" : ""}`}
              >
                <input
                  type="radio"
                  name="access-preset"
                  disabled={!option.available || busy || running}
                  checked={mode === option.mode}
                  onChange={() => {
                    setMode(option.mode);
                    setConfirmed(false);
                  }}
                />
                <span>
                  <span className="font-medium">{permissionLabels[option.mode]}</span>
                  <span className="mt-1 block text-xs text-muted-foreground">
                    {option.description}
                  </span>
                </span>
              </label>
            ))}
          </div>
          {mode === "full_access" && (
            <label className="flex gap-2 text-sm">
              <input
                type="checkbox"
                checked={confirmed}
                onChange={(event) => setConfirmed(event.target.checked)}
              />
              Allow unrestricted access within the server host policy.
            </label>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <Button
            type="button"
            disabled={busy || running || !permissions || (mode === "full_access" && !confirmed)}
            onClick={savePermissions}
          >
            Apply
          </Button>
        </DialogContent>
      </Dialog>
      {!session.draft && <SessionInspectorControls />}
      {error && !permissionOpen && (
        <span role="alert" className="text-destructive">
          {error}
        </span>
      )}
    </div>
  );
}

export function ComposerStatus() {
  const { session } = useChatSettings();
  const running = useAuiState((state) => state.thread.isRunning);
  if (session.draft && !running) return null;
  return (
    <ComposerPlan>
      <SessionFilesMenu />
    </ComposerPlan>
  );
}
