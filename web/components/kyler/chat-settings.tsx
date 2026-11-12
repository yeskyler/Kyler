"use client";

import { useEffect, useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import { ShieldIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogTrigger,
  DialogDescription,
} from "@/components/ui/dialog";
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
      className="min-w-0 max-w-full shrink px-3 py-2 text-left text-sm leading-5 break-all whitespace-normal text-muted-foreground"
      title={initialModel}
    >
      {initialModel}
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
    <div className="flex min-w-0 items-center gap-1 px-1 text-xs text-muted-foreground">
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
