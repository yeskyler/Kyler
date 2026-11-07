"use client";

import { useEffect, useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import {
  BlocksIcon,
  ChevronRightIcon,
  FolderIcon,
  FolderOpenIcon,
  SearchIcon,
  LoaderCircleIcon,
  RefreshCwIcon,
  Trash2Icon,
} from "lucide-react";
import { BrandMark } from "./brand-mark";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { api } from "@/lib/kyler/client";
import { WorkspacePicker } from "./workspace-picker";
import { Input } from "@/components/ui/input";
import {
  groupWorkspaceSessions,
  sessionActivity,
  sessionTitle,
  mergeSessionHistory,
} from "@/lib/kyler/sessions";
import type { Session, Workspace } from "@/lib/kyler/types";

type HistorySession = Session & { placeholder?: string };
type SessionPage = { sessions: Session[]; next_cursor: number | null };

export function WorkspaceSidebar({
  session,
  active,
  localSessions,
  defaultWorkspace,
  view,
  setView,
  createSession,
  selectSession,
  deleteSession,
}: {
  session: Session;
  active: boolean;
  localSessions: HistorySession[];
  defaultWorkspace?: string;
  view: "chat" | "resources";
  setView: (view: "chat" | "resources") => void;
  createSession: (workspace?: string | null) => Promise<void>;
  selectSession: (id: string) => void;
  deleteSession: (id: string, fallback?: string) => Promise<void>;
}) {
  const running = useAuiState((state) => state.thread.isRunning);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [sessions, setSessions] = useState<HistorySession[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [version, setVersion] = useState(0);
  const [deleting, setDeleting] = useState<HistorySession>();
  const [removingProject, setRemovingProject] = useState<Workspace>();
  const [expandedGroups, setExpandedGroups] = useState<Record<string, boolean>>({});
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    async function load() {
      const [page, projects] = await Promise.all([
        api<SessionPage>("sessions?limit=50", { signal: controller.signal }),
        api<Workspace[]>("workspaces", { signal: controller.signal }),
      ]);
      const items = await Promise.all(
        page.sessions.map(async (item) => {
          if (item.title?.trim()) return item;
          const first = await api<{ messages: { role: string; content: string }[] }>(
            `sessions/${item.session_id}/messages?limit=5`,
            { signal: controller.signal },
          ).catch(() => null);
          return {
            ...item,
            placeholder: first?.messages.find((message) => message.role === "user")?.content,
          };
        }),
      );
      if (!controller.signal.aborted) {
        setSessions(items);
        setWorkspaces(projects);
        setCursor(page.next_cursor);
        setError("");
      }
    }
    load().catch((error: Error) => {
      if (!controller.signal.aborted) setError(error.message);
    });
    return () => controller.abort();
  }, [active, session.session_id, session.draft, running, version]);

  // Refresh statuses without refetching message previews or collapsing loaded pages.
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    let pending = false;
    async function refresh() {
      if (pending || document.visibilityState === "hidden") return;
      pending = true;
      try {
        const updates = new Map<string, Session>();
        let cursor: number | null = 0;
        let count = 0;
        while (cursor !== null && (count === 0 || count < sessions.length)) {
          const page: SessionPage = await api(
            `sessions?limit=${Math.min(500, Math.max(50, sessions.length - count))}&cursor=${cursor}`,
            { signal: controller.signal },
          );
          for (const item of page.sessions) updates.set(item.session_id, item);
          count += page.sessions.length;
          cursor = page.next_cursor;
        }
        if (!controller.signal.aborted)
          setSessions((items) => {
            const known = new Set(items.map((item) => item.session_id));
            return [
              ...[...updates.values()].filter((item) => !known.has(item.session_id)),
              ...items.map((item) => ({ ...item, ...updates.get(item.session_id) })),
            ];
          });
      } catch (error) {
        if (!controller.signal.aborted) setError((error as Error).message);
      } finally {
        pending = false;
      }
    }
    const timer = setInterval(refresh, 3000);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      clearInterval(timer);
      controller.abort();
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [active, sessions.length, session.session_id]);

  async function more() {
    if (cursor === null) return;
    setBusy(true);
    try {
      const page = await api<SessionPage>(`sessions?limit=50&cursor=${cursor}`);
      setSessions((items) => [...items, ...page.sessions]);
      setCursor(page.next_cursor);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function newChat(workspace: string | null = session.workspace_root) {
    setBusy(true);
    setError("");
    try {
      await createSession(workspace);
      setView("chat");
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function remove() {
    if (!deleting) return;
    setBusy(true);
    setError("");
    try {
      const fallback = sessions.find((item) => item.session_id !== deleting.session_id)?.session_id;
      await deleteSession(deleting.session_id, fallback);
      setSessions((items) => items.filter((item) => item.session_id !== deleting.session_id));
      setDeleting(undefined);
      setVersion((value) => value + 1);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function removeProject() {
    if (!removingProject) return;
    setBusy(true);
    setError("");
    try {
      await api(`workspaces/${removingProject.id}`, { method: "DELETE" });
      const remaining = workspaces.filter((item) => item.id !== removingProject.id);
      setWorkspaces(remaining);
      if (session.workspace_root === removingProject.path) await createSession(null);
      setRemovingProject(undefined);
      setVersion((value) => value + 1);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  const visibleSessions = mergeSessionHistory(sessions, localSessions);
  const workspaceGroups = groupWorkspaceSessions(
    visibleSessions,
    session.workspace_root,
    workspaces,
    defaultWorkspace ?? null,
  );
  const deletingStatus = sessionActivity(
    visibleSessions.find((item) => item.session_id === deleting?.session_id)?.status ??
      deleting?.status,
    deleting?.session_id === session.session_id && running,
  );
  return (
    <aside
      className="flex h-full w-64 shrink-0 flex-col border-r border-sidebar-border/60 bg-sidebar px-3 py-4 text-sidebar-foreground sm:w-72"
      aria-label="Workspace navigation"
    >
      <div className="flex items-center gap-2.5 px-3 pt-2 pb-7 text-[28px] font-semibold tracking-tight">
        <BrandMark size={34} />
        <span>kyler</span>
      </div>
      <nav aria-label="Primary navigation" className="flex flex-col gap-1.5">
        <Button
          variant="ghost"
          className="h-11 w-full justify-center gap-2 rounded-xl bg-sidebar-accent px-3 text-sm font-medium text-sidebar-accent-foreground hover:bg-sidebar-accent"
          aria-current={view === "chat" && session.draft ? "page" : undefined}
          disabled={busy}
          onClick={() => void newChat()}
        >
          New Chat
        </Button>
        <Button
          variant="ghost"
          aria-current={view === "resources" ? "page" : undefined}
          className="h-11 w-full justify-start gap-3 rounded-xl px-3 text-sm font-medium hover:bg-sidebar-accent/60"
          onClick={() => setView("resources")}
        >
          <BlocksIcon className="size-4" />
          Plugins
        </Button>
      </nav>
      <div className="mt-8 mb-3 flex items-center justify-between pl-3 pr-1">
        <span className="text-xs font-medium text-sidebar-foreground/50">Workspaces</span>
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Search chats and workspaces"
            aria-pressed={searchOpen}
            className="text-sidebar-foreground/50"
            onClick={() => {
              setSearchOpen((value) => !value);
              setQuery("");
            }}
          >
            <SearchIcon className="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Refresh history"
            className="text-sidebar-foreground/50"
            onClick={() => setVersion((n) => n + 1)}
          >
            <RefreshCwIcon className="size-3.5" />
          </Button>
          <WorkspacePicker iconOnly onCreated={() => setVersion((value) => value + 1)} />
        </div>
      </div>
      {searchOpen && (
        <div className="mb-3 px-1">
          <Input
            autoFocus
            aria-label="Find a chat or workspace"
            placeholder="Find a chat or workspace"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            className="h-9 rounded-lg border-sidebar-border bg-background/60 text-xs"
          />
        </div>
      )}
      <nav
        aria-label="Workspace and chat history"
        className="min-h-0 flex-1 space-y-3 overflow-y-auto pb-3"
      >
        {workspaceGroups.map((group) => {
          const key = group.path ?? "__default__";
          const active = group.path === session.workspace_root;
          const matchesWorkspace = `${group.name} ${group.path ?? ""}`
            .toLowerCase()
            .includes(query.trim().toLowerCase());
          const items =
            query.trim() && !matchesWorkspace
              ? group.sessions.filter((item) =>
                  sessionTitle(item.title, item.placeholder)
                    .toLowerCase()
                    .includes(query.trim().toLowerCase()),
                )
              : group.sessions;
          if (query.trim() && !matchesWorkspace && !items.length) return null;
          const expanded = !!query.trim() || (expandedGroups[key] ?? active);
          return (
            <section key={key} aria-label={group.name}>
              <div className="group/project flex items-center rounded-lg hover:bg-sidebar-accent/70">
                <button
                  type="button"
                  aria-label={`${expanded ? "Collapse" : "Expand"} ${group.name}`}
                  aria-expanded={expanded}
                  onClick={() => setExpandedGroups((groups) => ({ ...groups, [key]: !expanded }))}
                  className="flex h-10 w-6 shrink-0 items-center justify-center rounded-lg text-sidebar-foreground/40 outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                >
                  <ChevronRightIcon
                    className={`size-3 transition-transform ${expanded ? "rotate-90" : ""}`}
                  />
                </button>
                <button
                  type="button"
                  disabled={busy}
                  aria-pressed={active}
                  title={group.path ?? "Default workspace"}
                  onClick={() => {
                    setExpandedGroups((groups) => ({ ...groups, [key]: true }));
                    if (!active) void newChat(group.path);
                    else setView("chat");
                  }}
                  className={`flex h-10 min-w-0 flex-1 items-center gap-2 rounded-lg pr-2 text-left text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-50 ${active ? "font-medium" : "text-sidebar-foreground/75"}`}
                >
                  {active ? (
                    <FolderOpenIcon className="size-[18px] shrink-0" />
                  ) : (
                    <FolderIcon className="size-[18px] shrink-0" />
                  )}
                  <span className="truncate">{group.name}</span>
                </button>
                {!group.sessions.length && workspaces.some((item) => item.path === group.path) && (
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    className="mr-1 shrink-0 text-sidebar-foreground/45 hover:text-destructive md:opacity-0 md:group-hover/project:opacity-100 md:group-focus-within/project:opacity-100 focus-visible:opacity-100"
                    disabled={busy || (active && running)}
                    aria-label={`Remove project ${group.name}`}
                    title="Remove empty project"
                    onClick={() => {
                      setError("");
                      setRemovingProject(workspaces.find((item) => item.path === group.path));
                    }}
                  >
                    <Trash2Icon className="size-3" />
                  </Button>
                )}
              </div>
              {expanded && (
                <div className="mt-1 space-y-0.5 pl-6">
                  {items.map((item) => {
                    const activity = sessionActivity(
                      item.status,
                      item.session_id === session.session_id && running,
                    );
                    const title = sessionTitle(item.title, item.placeholder);
                    return (
                      <div
                        key={item.session_id}
                        className="group flex items-center rounded-xl transition-colors hover:bg-sidebar-accent/70"
                      >
                        <button
                          type="button"
                          disabled={busy}
                          aria-current={
                            view === "chat" && item.session_id === session.session_id
                              ? "page"
                              : undefined
                          }
                          title={title}
                          onClick={() => {
                            if (item.session_id !== session.session_id)
                              selectSession(item.session_id);
                            setView("chat");
                          }}
                          className="flex min-w-0 flex-1 items-center gap-2 rounded-xl py-2.5 pl-3 pr-1 text-left text-[13px] disabled:opacity-50"
                        >
                          <span className="min-w-0 flex-1 truncate">{title}</span>
                          {activity.busy && (
                            <span
                              role="img"
                              aria-label={activity.label}
                              title={activity.label}
                              className="shrink-0 text-sidebar-foreground"
                            >
                              <LoaderCircleIcon
                                aria-hidden
                                className="size-3.5 animate-spin motion-reduce:animate-none"
                              />
                            </span>
                          )}
                        </button>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          className="mr-1 shrink-0 text-sidebar-foreground/45 hover:text-destructive md:opacity-0 md:group-hover:opacity-100 md:group-focus-within:opacity-100 focus-visible:opacity-100"
                          disabled={busy || activity.busy}
                          aria-label={`Delete ${title}`}
                          title={
                            activity.busy ? "Stop this session before deleting it" : "Delete chat"
                          }
                          onClick={() => {
                            setError("");
                            setDeleting(item);
                          }}
                        >
                          <Trash2Icon className="size-3" />
                        </Button>
                      </div>
                    );
                  })}
                  {!items.length && (
                    <p className="px-3 py-3 text-xs text-sidebar-foreground/45">
                      No chats in this workspace.
                    </p>
                  )}
                </div>
              )}
            </section>
          );
        })}
        {!workspaceGroups.length && (
          <p className="px-3 py-5 text-xs text-sidebar-foreground/50">
            Add a workspace to start a chat.
          </p>
        )}
        {cursor !== null && (
          <Button
            variant="ghost"
            size="sm"
            className="ml-3 text-sidebar-foreground/60"
            disabled={busy}
            onClick={more}
          >
            Load more
          </Button>
        )}
        {query.trim() &&
          !sessions.some((item) =>
            `${sessionTitle(item.title, item.placeholder)} ${item.workspace_root ?? "Default workspace"}`
              .toLowerCase()
              .includes(query.trim().toLowerCase()),
          ) && <p className="px-3 py-4 text-xs text-sidebar-foreground/50">No matching chats.</p>}
      </nav>
      {error && !deleting && !removingProject && (
        <p role="alert" className="my-2 px-2 text-xs text-destructive">
          {error}
        </p>
      )}
      <Dialog
        open={!!removingProject}
        onOpenChange={(open) => {
          if (!open && !busy) {
            setRemovingProject(undefined);
            setError("");
          }
        }}
      >
        <DialogContent>
          <DialogTitle>Remove project?</DialogTitle>
          <DialogDescription>
            Removes the empty project from Kyler. The local directory and its files are kept.
          </DialogDescription>
          <p className="rounded-lg bg-muted p-3 text-sm">{removingProject?.title}</p>
          {session.workspace_root === removingProject?.path && (
            <p className="text-xs text-muted-foreground">The current draft will be closed.</p>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button variant="outline" disabled={busy} onClick={() => setRemovingProject(undefined)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={busy || (running && session.workspace_root === removingProject?.path)}
              onClick={removeProject}
            >
              Remove project
            </Button>
          </div>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!deleting}
        onOpenChange={(open) => {
          if (!open && !busy) {
            setDeleting(undefined);
            setError("");
          }
        }}
      >
        <DialogContent>
          <DialogTitle>Delete chat?</DialogTitle>
          <DialogDescription>Chat and related files will be deleted.</DialogDescription>
          <p className="line-clamp-3 rounded-lg bg-muted p-3 text-sm">
            {sessionTitle(deleting?.title, deleting?.placeholder)}
          </p>
          {deletingStatus.busy && (
            <p className="text-sm text-muted-foreground">Stop this chat first.</p>
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button variant="outline" disabled={busy} onClick={() => setDeleting(undefined)}>
              Cancel
            </Button>
            <Button variant="destructive" disabled={busy || deletingStatus.busy} onClick={remove}>
              {busy ? "Deleting…" : "Delete"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </aside>
  );
}
