"use client";

import { useEffect, useState, type ReactNode } from "react";
import { useAuiState } from "@assistant-ui/react";
import { CheckIcon, ChevronRightIcon, ShieldIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { Menu } from "@base-ui/react/menu";
import { api } from "@/lib/kyler/client";
import {
  permissionLabels,
  type Permissions,
  type PermissionMode,
  type ModelCatalog,
  type SessionModel,
} from "@/lib/kyler/types";

import { useChatSettings } from "./chat-settings-context";
import { ComposerPlan } from "./composer-plan";
import { SessionFilesMenu } from "./artifact-browser";
import { SessionInspectorControls } from "./session-inspector";
export { ChatSettingsProvider } from "./chat-settings-context";

const thinkingLevels = ["off", "low", "high", "max"] as const;
type ThinkingLevel = (typeof thinkingLevels)[number];
type ThinkingState = {
  enabled: boolean;
  supported: boolean;
  level: ThinkingLevel;
  levels: ThinkingLevel[];
};
function normalizeThinking(value: ThinkingState): ThinkingState {
  return {
    ...value,
    level: thinkingLevels.includes(value.level) ? value.level : "high",
    levels: value.levels?.filter((level) => thinkingLevels.includes(level)) ?? [...thinkingLevels],
  };
}
const thinkingLabels: Record<ThinkingLevel, string> = {
  off: "Off",
  low: "Low",
  high: "High",
  max: "Max",
};

function SettingsSubmenu({
  label,
  value,
  children,
}: {
  label: string;
  value: string;
  children: ReactNode;
}) {
  return (
    <Menu.SubmenuRoot>
      <Menu.SubmenuTrigger className="flex min-h-9 w-full cursor-default items-center gap-3 rounded-lg px-3 py-2 text-sm outline-none data-[highlighted]:bg-accent">
        <span className="shrink-0">{label}</span>
        <span className="ml-auto min-w-0 truncate text-xs text-muted-foreground">{value}</span>
        <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
      </Menu.SubmenuTrigger>
      <Menu.Portal>
        <Menu.Positioner side="right" align="start" sideOffset={6} className="z-50">
          <Menu.Popup
            aria-label={label}
            className="max-h-[min(32rem,70vh)] w-64 max-w-[calc(100vw-2rem)] overflow-y-auto rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg outline-none"
          >
            {children}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.SubmenuRoot>
  );
}

export function ModelThinkingControl() {
  const { session, ensureSession, initialModel } = useChatSettings();
  const running = useAuiState((state) => state.thread.isRunning);
  const [catalog, setCatalog] = useState<ModelCatalog>();
  const [model, setModel] = useState<SessionModel>();
  const [thinking, setThinking] = useState<ThinkingState>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    setCatalog(undefined);
    setModel(undefined);
    setThinking(undefined);
    setError("");
    setOpen(false);
    ensureSession()
      .then(async (bound) => {
        const options = { signal: controller.signal };
        return Promise.all([
          api<ModelCatalog>("models", options),
          api<SessionModel>(`sessions/${bound.session_id}/model`, options),
          api<ThinkingState>(`sessions/${bound.session_id}/thinking`, options),
        ]);
      })
      .then(([models, selected, reasoning]) => {
        if (controller.signal.aborted) return;
        setCatalog(models);
        setModel(selected);
        setThinking(normalizeThinking(reasoning));
        setError(selected.error ?? "");
      })
      .catch(() => {
        if (!controller.signal.aborted) setError("Unable to load model and thinking settings.");
      });
    return () => controller.abort();
  }, [session.session_id, ensureSession]);

  async function selectModel(profile: string) {
    if (busy || running || !catalog?.models.some((entry) => entry.profile === profile)) return;
    setOpen(false);
    setBusy(true);
    setError("");
    try {
      const bound = await ensureSession();
      const selected = await api<SessionModel>(`sessions/${bound.session_id}/model`, {
        method: "PUT",
        body: JSON.stringify({ profile }),
      });
      setModel(selected);
      setError(selected.error ?? "");
      setThinking(
        normalizeThinking(await api<ThinkingState>(`sessions/${bound.session_id}/thinking`)),
      );
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function selectThinking(level: ThinkingLevel) {
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
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  const modelName = model?.selected?.model ?? initialModel;
  const label = thinkingLabels[thinking?.level ?? "high"];
  return (
    <span className="min-w-0 shrink">
      <Menu.Root open={open} onOpenChange={setOpen}>
        <Menu.Trigger
          render={
            <Button
              type="button"
              variant="ghost"
              size="xs"
              disabled={running || busy || !catalog || !thinking}
              aria-label={`Model and thinking: ${modelName} ${label}`}
              aria-busy={busy}
              title={error || `${modelName} · ${label}`}
              className={`max-w-[55vw] gap-2 px-2 font-normal ${error ? "text-destructive" : "text-muted-foreground hover:text-foreground"}`}
            />
          }
        >
          <span className="min-w-0 truncate">{modelName}</span>
          <span className="shrink-0">{label}</span>
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner side="top" align="end" sideOffset={8} className="z-50">
            <Menu.Popup
              className="max-h-[min(32rem,70vh)] w-72 max-w-[calc(100vw-2rem)] overflow-y-auto rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg outline-none"
              aria-label="Model and thinking settings"
            >
              <SettingsSubmenu label="Model" value={modelName}>
                <Menu.RadioGroup aria-label="Model" value={model?.selected?.profile ?? ""}>
                  {catalog?.models.map((entry) => (
                    <Menu.RadioItem
                      key={entry.profile}
                      value={entry.profile}
                      title={`${entry.profile} · ${entry.provider}`}
                      onClick={() => void selectModel(entry.profile)}
                      disabled={busy || running}
                      className="flex min-h-9 cursor-default items-center justify-between gap-3 rounded-lg px-3 py-2 text-sm outline-none data-[highlighted]:bg-accent data-[disabled]:opacity-40"
                    >
                      <span className="min-w-0 truncate">{entry.model}</span>
                      <Menu.RadioItemIndicator>
                        <CheckIcon className="size-3.5 shrink-0" aria-hidden />
                      </Menu.RadioItemIndicator>
                    </Menu.RadioItem>
                  ))}
                </Menu.RadioGroup>
              </SettingsSubmenu>
              <SettingsSubmenu label="Thinking level" value={label}>
                <Menu.RadioGroup aria-label="Thinking level" value={thinking?.level ?? ""}>
                  {thinkingLevels.map((level) => (
                    <Menu.RadioItem
                      key={level}
                      value={level}
                      onClick={() => void selectThinking(level)}
                      disabled={busy || running || !thinking?.levels.includes(level)}
                      className="flex min-h-9 cursor-default items-center justify-between gap-3 rounded-lg px-3 py-2 text-sm outline-none data-[highlighted]:bg-accent data-[disabled]:opacity-40"
                    >
                      {thinkingLabels[level]}
                      <Menu.RadioItemIndicator>
                        <CheckIcon className="size-3.5" aria-hidden />
                      </Menu.RadioItemIndicator>
                    </Menu.RadioItem>
                  ))}
                </Menu.RadioGroup>
              </SettingsSubmenu>
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

export function PermissionControl() {
  const { session, ensureSession } = useChatSettings();
  const running = useAuiState((state) => state.thread.isRunning);
  const [permissions, setPermissions] = useState<Permissions>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [permissionOpen, setPermissionOpen] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  useEffect(() => {
    if (!menuOpen) return;
    let disposed = false;
    ensureSession()
      .then((bound) => api<Permissions>(`sessions/${bound.session_id}/permissions`))
      .then((value) => {
        if (!disposed) {
          setPermissions(value);
        }
      })
      .catch((error: Error) => {
        if (!disposed) setError(error.message);
      });
    return () => {
      disposed = true;
    };
  }, [session.session_id, menuOpen, ensureSession]);
  async function savePermissions(mode: PermissionMode, confirmFullAccess = false) {
    if (busy || running) return;
    setBusy(true);
    setError("");
    try {
      const bound = await ensureSession();
      setPermissions(
        await api<Permissions>(`sessions/${bound.session_id}/permissions`, {
          method: "PUT",
          body: JSON.stringify({
            mode,
            confirm_full_access: mode === "full_access" && confirmFullAccess,
          }),
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
    <div className="min-w-0 shrink-0 text-xs text-muted-foreground">
      <Menu.Root
        open={menuOpen}
        onOpenChange={(open) => {
          setError("");
          setMenuOpen(open);
        }}
      >
        <Menu.Trigger
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
              disabled={!session.workspace_root || busy || running}
              aria-busy={busy}
            />
          }
        >
          <ShieldIcon />
          <span className="hidden sm:inline">
            {permissions ? permissionLabels[permissions.selected.mode] : "Permissions"}
          </span>
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner side="top" align="start" sideOffset={8} className="z-50">
            <Menu.Popup
              aria-label="Sandbox permissions"
              className="min-w-44 max-w-[calc(100vw-2rem)] rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg outline-none"
            >
              {!permissions && (
                <p className="px-3 py-2 text-sm text-muted-foreground">
                  {error || "Loading permissions…"}
                </p>
              )}
              <Menu.RadioGroup value={permissions?.selected.mode ?? ""}>
                {permissions?.options.map((option) => (
                  <Menu.RadioItem
                    key={option.mode}
                    value={option.mode}
                    disabled={!option.available || busy || running}
                    onClick={() => {
                      if (
                        !option.available ||
                        busy ||
                        running ||
                        option.mode === permissions.selected.mode
                      )
                        return;
                      setMenuOpen(false);
                      setError("");
                      if (option.mode === "full_access") {
                        setConfirmed(false);
                        setPermissionOpen(true);
                      } else {
                        void savePermissions(option.mode);
                      }
                    }}
                    className="flex min-h-9 cursor-default items-center justify-between gap-3 rounded-lg px-3 py-2 text-sm outline-none data-[highlighted]:bg-accent data-[disabled]:opacity-40"
                  >
                    <span>{permissionLabels[option.mode]}</span>
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
      <Dialog
        open={permissionOpen}
        onOpenChange={(open) => {
          if (busy) return;
          setError("");
          setPermissionOpen(open);
          setConfirmed(false);
        }}
      >
        <DialogContent>
          <DialogTitle>Enable full access?</DialogTitle>
          <DialogDescription>
            Enabling full access skips routine tool approvals and allows the agent to modify files,
            execute commands, and perform sensitive operations beyond the current workspace. Use it
            only when you trust the current task. Operating system permissions still apply.
          </DialogDescription>
          <label className="flex gap-2 text-sm">
            <input
              type="checkbox"
              checked={confirmed}
              disabled={busy}
              onChange={(event) => setConfirmed(event.target.checked)}
            />
            I understand the risks and allow full access for this session.
          </label>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => setPermissionOpen(false)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              disabled={busy || running || !permissions || !confirmed}
              onClick={() => void savePermissions("full_access", confirmed)}
            >
              Enable full access
            </Button>
          </div>
        </DialogContent>
      </Dialog>
      {error && !permissionOpen && (
        <span role="alert" className="text-destructive">
          {error}
        </span>
      )}
    </div>
  );
}

export function ComposerSettings() {
  const { session } = useChatSettings();
  if (session.draft) return null;
  return (
    <div className="flex w-full min-w-0 items-center px-1 text-xs text-muted-foreground">
      <SessionInspectorControls />
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
