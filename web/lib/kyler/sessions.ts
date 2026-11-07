export function sessionActivity(status?: string | null, localRunning = false) {
  if (localRunning && status !== "cancelling")
    return { kind: "active", label: "Working", busy: true } as const;
  switch (status) {
    case "starting":
      return { kind: "active", label: "Starting", busy: true } as const;
    case "running":
      return { kind: "active", label: "Working", busy: true } as const;
    case "cancelling":
      return { kind: "active", label: "Stopping", busy: true } as const;
    case "completed":
      return { kind: "completed", label: "Completed", busy: false } as const;
    case "failed":
      return { kind: "failed", label: "Failed", busy: false } as const;
    case "cancelled":
      return { kind: "cancelled", label: "Cancelled", busy: false } as const;
    default:
      return { kind: "idle", label: "Idle", busy: false } as const;
  }
}

export function workspaceName(path: string | null) {
  if (path === "/") return "Root workspace";
  return path?.split("/").filter(Boolean).at(-1) ?? "Default workspace";
}

export function groupWorkspaceSessions<T extends { workspace_root: string | null }>(
  sessions: T[],
  current: string | null,
  workspaces: { path: string; title: string }[] = [],
  defaultWorkspace: string | null = null,
) {
  const groups = new Map<string | null, { path: string | null; name: string; sessions: T[] }>();
  const titles = new Map(workspaces.map((workspace) => [workspace.path, workspace.title]));
  groups.set(defaultWorkspace, { path: defaultWorkspace, name: "Default workspace", sessions: [] });
  if (current !== null && current !== defaultWorkspace)
    groups.set(current, {
      path: current,
      name: titles.get(current ?? "") ?? workspaceName(current),
      sessions: [],
    });
  for (const workspace of workspaces)
    if (!groups.has(workspace.path))
      groups.set(workspace.path, { path: workspace.path, name: workspace.title, sessions: [] });
  for (const session of sessions) {
    const path = session.workspace_root;
    if (!groups.has(path))
      groups.set(path, { path, name: titles.get(path ?? "") ?? workspaceName(path), sessions: [] });
    groups.get(path)!.sessions.push(session);
  }
  return [...groups.values()];
}

export function sessionTitle(title?: string | null, input?: string | null) {
  return (title?.trim() || input?.trim() || "New chat").replace(/\s+/g, " ").slice(0, 90);
}

/** Include pending local runs without publishing empty drafts to history. */
export function mergeSessionHistory<
  T extends {
    session_id: string;
    draft?: boolean;
    status?: string | null;
    title?: string | null;
    placeholder?: string;
  },
>(saved: T[], local: T[]): T[] {
  const items = new Map(saved.map((session) => [session.session_id, session]));
  for (const session of local) {
    const previous = items.get(session.session_id);
    if (previous)
      items.set(session.session_id, {
        ...previous,
        placeholder: previous.placeholder ?? session.placeholder,
        ...(session.status === "running" ? { status: "running" } : {}),
      });
    else if (session.placeholder?.trim() && (!session.draft || session.status === "running"))
      items.set(session.session_id, session);
  }
  return [...items.values()];
}
