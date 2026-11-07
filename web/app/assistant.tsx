"use client";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AssistantRuntimeProvider,
  useLocalRuntime,
  type ThreadMessageLike,
} from "@assistant-ui/react";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { Button } from "@/components/ui/button";
import {
  api,
  attachments,
  chatAdapter,
  pendingSessionKey,
  selectedSessionKey,
  runStorageKey,
} from "@/lib/kyler/client";
import { loadHistory } from "@/lib/kyler/history";
import { createDraftSession, discardDraft } from "@/lib/kyler/drafts";
import {
  SessionInspectorProvider,
  SessionInspectorPage,
  SessionInspectorBackButton,
} from "@/components/kyler/session-inspector";
import {
  ChatSettingsProvider,
  ComposerSettings,
  ComposerStatus,
  ModelControl,
} from "@/components/kyler/chat-settings";
import { ArtifactsProvider, ArtifactBrowserLayout } from "@/components/kyler/artifacts";
import { QuestionCard, QuestionsProvider, useQuestions } from "@/components/kyler/questions";
import { ApprovalCard } from "@/components/kyler/approval-card";
import { AssistantMessage } from "@/components/kyler/assistant-message";
import { ComposerMenu } from "@/components/kyler/composer-menu";
import { SidebarToggle } from "@/components/kyler/sidebar-toggle";
import { WorkspaceSidebar } from "@/components/kyler/workspace-sidebar";
import { ResourcesPanel } from "@/components/kyler/resources-panel";
import type { Session } from "@/lib/kyler/types";
import type { Approval } from "@/lib/kyler/events";

type Loaded = {
  defaultWorkspace?: string;
  initialText?: string;
  key: string;
  session: Session;
  messages: ThreadMessageLike[];
  model: string;
  approvals: Approval[];
  recoverRun?: string;
};
const storageKey = selectedSessionKey;
type Info = { model: string; default_workspace?: string; workspace_root?: string | null };

export function Assistant() {
  const [loaded, updateLoaded] = useState<Loaded>();
  const [opened, setOpened] = useState<Record<string, Loaded>>({});
  const [live, setLive] = useState<Record<string, Session & { placeholder?: string }>>({});
  const setLoaded = (value: Loaded | undefined) => {
    if (value) setOpened((items) => ({ ...items, [value.key]: value }));
    updateLoaded(value);
  };
  const reportSession = useCallback((session: Session & { placeholder?: string }) => {
    setLive((items) => {
      const previous = items[session.session_id];
      if (
        previous?.status === session.status &&
        previous?.placeholder === session.placeholder &&
        previous?.draft === session.draft
      )
        return items;
      return { ...items, [session.session_id]: session };
    });
  }, []);
  const [error, setError] = useState("");
  const [generation, setGeneration] = useState(0);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  useEffect(() => {
    try {
      setSidebarCollapsed(localStorage.getItem("kyler.sidebar.collapsed") === "true");
    } catch {
      // Start expanded if browser storage is unavailable.
    }
  }, []);
  const changeSidebarCollapsed = (collapsed: boolean) => {
    setSidebarCollapsed(collapsed);
    try {
      localStorage.setItem("kyler.sidebar.collapsed", String(collapsed));
    } catch {
      // Keep the current layout without persistence.
    }
  };
  const infoRef = useRef<Info | null>(null);
  const activeKey = useRef<string | null>(null);
  const loading = useRef(0);
  useEffect(() => {
    let disposed = false;
    const sequence = ++loading.current;
    async function load() {
      const info = await api<Info>("info");
      infoRef.current = info;
      const saved = localStorage.getItem(pendingSessionKey) ?? localStorage.getItem(storageKey);
      if (!saved) {
        const session = await createDraftSession();
        if (!disposed && sequence === loading.current) {
          activeKey.current = session.session_id;
          setLoaded({
            key: session.session_id,
            defaultWorkspace: info.default_workspace ?? info.workspace_root ?? undefined,
            session,
            messages: [],
            model: info.model,
            approvals: [],
          });
        } else void discardDraft(session.session_id);
        return;
      }
      const session = await api<Session>(`sessions/${encodeURIComponent(saved)}`);
      let recoverRun: string | undefined;
      let recoverExecution: string | null = null;
      let recoverInput = "";
      const pending = localStorage.getItem(runStorageKey(session.session_id));
      if (pending) {
        const stored = JSON.parse(pending) as { id: string; input: string };
        const run = await api<{ status: string; execution_id: string | null }>(`runs/${stored.id}`);
        if (["starting", "running", "cancelling"].includes(run.status)) {
          recoverRun = stored.id;
          recoverExecution = run.execution_id;
          recoverInput = stored.input;
        } else localStorage.removeItem(runStorageKey(session.session_id));
      }
      const messages = await loadHistory(session.session_id, recoverExecution);
      if (recoverRun && messages.at(-1)?.role !== "user")
        messages.push({ id: `recover-${recoverRun}`, role: "user", content: recoverInput });
      const model = await api<{
        selected: { model: string } | null;
        active: { model: string } | null;
        error: string | null;
      }>(`sessions/${session.session_id}/model`);
      if (model.error) throw new Error(model.error);
      const approvals = await api<Approval[]>(`sessions/${session.session_id}/approvals`);
      if (!disposed && sequence === loading.current) {
        activeKey.current = session.session_id;
        if (!session.draft) {
          localStorage.setItem(storageKey, session.session_id);
          localStorage.removeItem(pendingSessionKey);
        }
        setLoaded({
          key: session.session_id,
          defaultWorkspace: info.default_workspace ?? info.workspace_root ?? undefined,
          session,
          messages,
          model: model.active?.model ?? model.selected?.model ?? info.model,
          approvals,
          recoverRun,
        });
      }
    }
    load().catch((error: Error) => {
      if (!disposed && sequence === loading.current) setError(error.message);
    });
    return () => {
      disposed = true;
    };
  }, [generation]);
  const releaseDraft = () => {
    if (!loaded?.session.draft || live[loaded.key]?.status === "running") return;
    void discardDraft(loaded.key);
    setOpened((items) =>
      Object.fromEntries(Object.entries(items).filter(([key]) => key !== loaded.key)),
    );
    setLive((items) =>
      Object.fromEntries(Object.entries(items).filter(([key]) => key !== loaded.key)),
    );
  };
  const selectSession = (id: string) => {
    if (loaded?.key !== id) releaseDraft();
    const cached = opened[id];
    if (cached) {
      ++loading.current;
      activeKey.current = id;
      localStorage.setItem(storageKey, id);
      localStorage.removeItem(pendingSessionKey);
      setError("");
      updateLoaded(cached);
      return;
    }
    activeKey.current = null;
    localStorage.removeItem(pendingSessionKey);
    localStorage.setItem(storageKey, id);
    setLoaded(undefined);
    setError("");
    setGeneration((n) => n + 1);
  };
  const createSession = async (workspace?: string | null, initialText = "") => {
    const sequence = ++loading.current;
    const info = await api<Info>("info").catch((error) => {
      if (infoRef.current) return infoRef.current;
      throw error;
    });
    const session = await createDraftSession(workspace);
    if (sequence !== loading.current) {
      void discardDraft(session.session_id);
      return;
    }
    infoRef.current = info;
    releaseDraft();
    activeKey.current = session.session_id;
    localStorage.removeItem(storageKey);
    localStorage.removeItem(pendingSessionKey);
    setError("");
    setLoaded({
      key: session.session_id,
      defaultWorkspace: info.default_workspace ?? info.workspace_root ?? undefined,
      initialText,
      session,
      messages: [],
      model: info.model,
      approvals: [],
    });
  };
  const admit = (key: string, id: string) => {
    if (activeKey.current === key) localStorage.setItem(pendingSessionKey, id);
  };
  const publish = (key: string, id: string) => {
    const promote = (current: Loaded) => ({
      ...current,
      session: { ...current.session, session_id: id, draft: false },
    });
    setOpened((items) => (items[key] ? { ...items, [key]: promote(items[key]) } : items));
    updateLoaded((current) => (current?.key === key ? promote(current) : current));
    if (activeKey.current === key) {
      localStorage.setItem(storageKey, id);
      localStorage.removeItem(pendingSessionKey);
    }
  };
  const deleteSession = async (id: string, fallback?: string) => {
    await api(`sessions/${id}`, { method: "DELETE" });
    localStorage.removeItem(runStorageKey(id));
    setOpened((items) => Object.fromEntries(Object.entries(items).filter(([key]) => key !== id)));
    setLive((items) => Object.fromEntries(Object.entries(items).filter(([key]) => key !== id)));
    if (loaded?.session.session_id === id) {
      if (fallback) selectSession(fallback);
      else await createSession(loaded.session.workspace_root ?? undefined);
    }
  };
  const notice = error ? (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-4 p-8">
      <h1 className="text-xl font-semibold">Kyler</h1>
      <p role="alert">{error}</p>
      <div className="flex gap-2">
        <Button
          onClick={() => {
            setError("");
            setGeneration((n) => n + 1);
          }}
        >
          Retry
        </Button>
        <Button
          variant="outline"
          onClick={() => createSession().catch((error: Error) => setError(error.message))}
        >
          New chat
        </Button>
      </div>
    </main>
  ) : !loaded ? (
    <main className="flex min-h-dvh items-center justify-center text-muted-foreground">
      Connecting to Kyler…
    </main>
  ) : null;
  return (
    <>
      {notice}
      {Object.values(opened).map((item) => (
        <div
          key={item.key}
          hidden={loaded?.key !== item.key || !!error}
          inert={loaded?.key !== item.key || !!error}
        >
          <Connected
            loaded={item}
            active={loaded?.key === item.key && !error}
            localSessions={Object.values(live)}
            reportSession={reportSession}
            sidebarCollapsed={sidebarCollapsed}
            changeSidebarCollapsed={changeSidebarCollapsed}
            createSession={createSession}
            selectSession={selectSession}
            deleteSession={deleteSession}
            publish={publish}
            admit={admit}
          />
        </div>
      ))}
    </>
  );
}

function Connected({
  loaded,
  active,
  localSessions,
  reportSession,
  sidebarCollapsed,
  changeSidebarCollapsed,
  createSession,
  selectSession,
  deleteSession,
  publish,
  admit,
}: {
  loaded: Loaded;
  active: boolean;
  localSessions: (Session & { placeholder?: string })[];
  reportSession: (session: Session & { placeholder?: string }) => void;
  sidebarCollapsed: boolean;
  changeSidebarCollapsed: (collapsed: boolean) => void;
  createSession: (workspace?: string | null, initialText?: string) => Promise<void>;
  selectSession: (id: string) => void;
  deleteSession: (id: string, fallback?: string) => Promise<void>;
  publish: (key: string, id: string) => void;
  admit: (key: string, id: string) => void;
}) {
  const [view, setView] = useState<"chat" | "resources" | "inspection">("chat");
  const [mobileNavigation, setMobileNavigation] = useState(false);
  const navigation = useRef<HTMLDivElement>(null);
  const expandSidebar = useRef<HTMLButtonElement>(null);
  const mobileMenu = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const media = window.matchMedia("(min-width: 768px)");
    const update = () => {
      if (media.matches) setMobileNavigation(false);
    };
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  const closeMobileNavigation = () => {
    setMobileNavigation(false);
    requestAnimationFrame(() => mobileMenu.current?.focus());
  };
  useEffect(() => {
    if (!mobileNavigation) return;
    const close = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setMobileNavigation(false);
        mobileMenu.current?.focus();
      }
    };
    document.addEventListener("keydown", close);
    return () => document.removeEventListener("keydown", close);
  }, [mobileNavigation]);
  const questions = useQuestions(loaded.session.session_id);
  const pendingQuestion = questions.records.find((record) => record.status === "pending");
  const [approvals, setApprovals] = useState(
    loaded.approvals.filter((item) => item.status === "pending"),
  );
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const bound = useRef(Promise.resolve(loaded.session));
  const saved = useRef(!loaded.session.draft);
  const ensureSession = useCallback(() => bound.current, []);
  const adapter = useMemo(
    () =>
      chatAdapter(
        ensureSession,
        (approval) => {
          setApprovals((items) => [
            ...items.filter((item) => item.id !== approval.id),
            ...(approval.status === "pending" ? [approval] : []),
          ]);
        },
        loaded.recoverRun,
        {
          onQuestion: questions.update,
          onAdmitted: (id) => {
            if (!saved.current) admit(loaded.key, id);
          },
          onResponse: (id) => {
            if (!saved.current) {
              saved.current = true;
              publish(loaded.key, id);
            }
          },
        },
      ),
    [ensureSession, loaded.key, loaded.recoverRun, questions.update],
  );
  const attachmentAdapter = useMemo(() => attachments(ensureSession), [ensureSession]);
  const runtime = useLocalRuntime(adapter, {
    initialMessages: loaded.messages,
    maxSteps: 1,
    adapters: { attachments: attachmentAdapter },
  });
  useEffect(() => {
    const report = () => {
      const state = runtime.thread.getState();
      const input = state.messages.find((message) => message.role === "user");
      reportSession({
        ...loaded.session,
        status: state.isRunning ? "running" : "idle",
        placeholder: input?.content
          .filter((part) => part.type === "text")
          .map((part) => part.text)
          .join(" "),
      });
    };
    report();
    return runtime.thread.subscribe(report);
  }, [runtime, loaded.session, reportSession]);
  const recoveryStarted = useRef(false);
  useEffect(() => {
    if (recoveryStarted.current) return;
    recoveryStarted.current = true;
    if (loaded.recoverRun)
      runtime.thread.startRun({ parentId: runtime.thread.getState().messages.at(-1)?.id ?? null });
    else if (loaded.initialText) runtime.thread.composer.setText(loaded.initialText);
    // Runtime owns this subscription after mount; never create a second run during replay.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const decide = async (approval: Approval, decision: string) => {
    setBusy(true);
    setError("");
    try {
      await api(`approvals/${approval.id}/decision`, {
        method: "POST",
        body: JSON.stringify({ decision, digest: approval.digest }),
      });
      setApprovals((items) => items.filter((item) => item.id !== approval.id));
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <QuestionsProvider records={questions.records}>
        {/* The provider owns runtime lifecycle; only foreground views may unmount. */}
        {active && (
          <ChatSettingsProvider
            session={loaded.session}
            initialModel={loaded.model}
            createSession={(workspace) =>
              createSession(workspace, runtime.thread.composer.getState().text)
            }
            startSkillCreation={() => createSession(null, "@skill-creator ")}
            ensureSession={ensureSession}
          >
            <ArtifactsProvider session={loaded.session.session_id}>
              <SessionInspectorProvider
                session={loaded.session.session_id}
                active={active && view === "inspection"}
                enabled={!loaded.session.draft}
                onOpen={() => setView("inspection")}
                onBack={() => setView("chat")}
              >
                <main className="relative flex h-dvh overflow-hidden">
                  {mobileNavigation && (
                    <button
                      type="button"
                      aria-label="Close navigation"
                      className="fixed inset-x-0 bottom-0 top-12 z-40 bg-black/30 md:hidden"
                      onClick={closeMobileNavigation}
                    />
                  )}
                  <div
                    id={`workspace-navigation-${loaded.key}`}
                    ref={navigation}
                    inert={sidebarCollapsed && !mobileNavigation}
                    aria-hidden={sidebarCollapsed && !mobileNavigation ? true : undefined}
                    className={`${mobileNavigation ? "absolute bottom-0 top-12 left-0 z-50 flex shadow-xl" : "hidden"} overflow-hidden bg-sidebar md:relative md:flex md:shrink-0 md:shadow-none md:transition-[width] md:duration-[180ms] md:ease-in-out motion-reduce:transition-none ${sidebarCollapsed ? "md:w-0" : "md:w-72"}`}
                  >
                    <WorkspaceSidebar
                      session={loaded.session}
                      active={active}
                      localSessions={localSessions}
                      defaultWorkspace={loaded.defaultWorkspace}
                      view={view === "inspection" ? "chat" : view}
                      setView={(next) => {
                        setView(next);
                        setMobileNavigation(false);
                      }}
                      createSession={createSession}
                      selectSession={selectSession}
                      deleteSession={deleteSession}
                    />
                  </div>
                  <div className="flex min-w-0 flex-1 flex-col">
                    <header className="flex h-12 shrink-0 items-center justify-between px-4">
                      <div className="flex min-w-0 items-center gap-3">
                        <SidebarToggle
                          ref={mobileMenu}
                          direction={mobileNavigation ? "collapse" : "expand"}
                          className="md:hidden"
                          aria-label={mobileNavigation ? "Collapse sidebar" : "Expand sidebar"}
                          aria-controls={`workspace-navigation-${loaded.key}`}
                          aria-expanded={mobileNavigation}
                          onClick={() => setMobileNavigation(!mobileNavigation)}
                        />
                        <SidebarToggle
                          ref={expandSidebar}
                          direction={sidebarCollapsed ? "expand" : "collapse"}
                          className="hidden md:inline-flex"
                          aria-controls={`workspace-navigation-${loaded.key}`}
                          aria-expanded={!sidebarCollapsed}
                          onClick={() => changeSidebarCollapsed(!sidebarCollapsed)}
                        />
                        <SessionInspectorBackButton />
                        {view !== "inspection" && (
                          <h1 className="truncate text-sm font-medium text-foreground">
                            {view === "resources"
                              ? "Plugins"
                              : loaded.session.draft
                                ? "New Chat"
                                : loaded.session.title || "Chat"}
                          </h1>
                        )}
                      </div>
                    </header>
                    <div className={view === "chat" ? "relative min-h-0 flex-1" : "hidden"}>
                      <ArtifactBrowserLayout enabled={!loaded.session.draft}>
                        <Thread
                          autoFocus={active}
                          composerBefore={
                            approvals[0] ? (
                              <ApprovalCard
                                key={approvals[0].id}
                                approval={approvals[0]}
                                count={approvals.length}
                                busy={busy}
                                error={error}
                                onDecision={(decision) => decide(approvals[0], decision)}
                              />
                            ) : pendingQuestion ? (
                              <QuestionCard
                                key={pendingQuestion.id}
                                record={pendingQuestion}
                                session={loaded.session.session_id}
                                onUpdate={questions.update}
                              />
                            ) : null
                          }
                          components={{
                            Welcome: NewChatWelcome,
                            AssistantMessage,
                            ComposerExtra: ModelControl,
                            ComposerMenu,
                            ComposerHeader: ComposerStatus,
                            ComposerFooter: ComposerSettings,
                          }}
                        />
                      </ArtifactBrowserLayout>
                    </div>
                    <div className={view === "inspection" ? "min-h-0 flex-1" : "hidden"}>
                      <SessionInspectorPage />
                    </div>
                    {view === "resources" && (
                      <div className="min-h-0 flex-1">
                        <ResourcesPanel />
                      </div>
                    )}
                  </div>
                </main>
              </SessionInspectorProvider>
            </ArtifactsProvider>
          </ChatSettingsProvider>
        )}
      </QuestionsProvider>
    </AssistantRuntimeProvider>
  );
}

function NewChatWelcome() {
  return (
    <div className="mb-7 px-3 text-center">
      <h2 className="text-2xl font-medium tracking-tight sm:text-[28px] sm:leading-tight">
        What can <span className="font-semibold">kyler</span> help you with?
      </h2>
      <p className="mt-2.5 text-sm text-muted-foreground">Research, build, or explore an idea.</p>
    </div>
  );
}
