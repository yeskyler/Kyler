"use client";

import { useEffect, useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import { ShieldIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog,
  DialogContent,
  DialogTitle,
  DialogTrigger,
  DialogDescription,
} from "@/components/ui/dialog";
import { api } from "@/lib/kyler/client";
import {
  permissionLabels,
  type ModelConnectionSettings,
  type Permissions,
  type PermissionMode,
} from "@/lib/kyler/types";

import { useChatSettings } from "./chat-settings-context";
import { ComposerPlan } from "./composer-plan";
import { SessionFilesMenu } from "./artifact-browser";
import { SessionInspectorControls } from "./session-inspector";
import { WorkspacePicker } from "./workspace-picker";
export { ChatSettingsProvider } from "./chat-settings-context";

export function ModelControl() {
  const { session, initialModel } = useChatSettings();
  const connectionPath = session.session_id.startsWith("draft-")
    ? "models/settings"
    : `sessions/${session.session_id}/model/settings`;
  const running = useAuiState((state) => state.thread.isRunning);
  const [settings, setSettings] = useState<ModelConnectionSettings>();
  const [url, setUrl] = useState("");
  const [model, setModel] = useState("");
  const [key, setKey] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!open) return;
    let disposed = false;
    setLoading(true);
    api<ModelConnectionSettings>(connectionPath)
      .then((value) => {
        if (!disposed) {
          setSettings(value);
          setUrl(value.url);
          setModel(value.model);
          setError("");
        }
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
  }, [connectionPath, open]);
  async function save() {
    setBusy(true);
    setError("");
    try {
      const value = await api<ModelConnectionSettings>(connectionPath, {
        method: "PUT",
        body: JSON.stringify({
          url: url.trim(),
          model: model.trim(),
          ...(key.trim() ? { key: key.trim() } : {}),
        }),
      });
      setSettings(value);
      setUrl(value.url);
      setModel(value.model);
      setKey("");
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
        if (!busy) {
          setOpen(value);
          setKey("");
        }
      }}
    >
      <DialogTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-auto min-w-0 max-w-full shrink whitespace-normal text-left leading-5 text-muted-foreground"
          />
        }
      >
        <span className="break-all">{settings?.model ?? initialModel}</span>
      </DialogTrigger>
      <DialogContent>
        <DialogTitle>Model settings</DialogTitle>
        <DialogDescription>
          OpenAI-compatible Chat Completions. Saved to the selected Kyler profile and applied to the
          next turn.
        </DialogDescription>
        <label className="space-y-1.5 text-sm" htmlFor="model-url">
          URL
          <Input
            id="model-url"
            type="url"
            value={url}
            disabled={loading || busy}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="https://api.openai.com/v1"
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        <label className="space-y-1.5 text-sm" htmlFor="model-name">
          Model
          <Input
            id="model-name"
            value={model}
            disabled={loading || busy}
            maxLength={256}
            onChange={(event) => setModel(event.target.value)}
            placeholder="Model identifier"
            autoComplete="off"
            spellCheck={false}
          />
        </label>
        <label className="space-y-1.5 text-sm" htmlFor="model-key">
          Key
          <Input
            id="model-key"
            type="password"
            value={key}
            disabled={loading || busy}
            maxLength={4096}
            onChange={(event) => setKey(event.target.value)}
            placeholder={
              settings?.key_configured ? "Leave blank to keep the current key" : "Enter API key"
            }
            autoComplete="new-password"
            spellCheck={false}
          />
        </label>
        <p className="text-xs text-muted-foreground">
          Use the API base URL. Existing keys are never displayed.
        </p>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        <Button
          type="button"
          disabled={
            loading ||
            busy ||
            running ||
            !settings ||
            !url.trim() ||
            !model.trim() ||
            (!settings.key_configured && !key.trim())
          }
          onClick={save}
        >
          {busy ? "Saving…" : "Save"}
        </Button>
      </DialogContent>
    </Dialog>
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
