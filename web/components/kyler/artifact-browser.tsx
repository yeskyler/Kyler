"use client";

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useAuiState } from "@assistant-ui/react";
import {
  ChevronLeftIcon,
  ChevronRightIcon,
  DownloadIcon,
  MonitorIcon,
  FolderTreeIcon,
  XIcon,
  RefreshCwIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { loadWorkspaceFiles } from "@/lib/kyler/workspace-files";
import {
  autoPreview,
  displayPreviewFiles,
  selectedPreviewFile,
  previewPath,
  initialBrowserState,
  previewFiles,
  restoreBrowserState,
  type BrowserState,
  type PreviewFile,
} from "@/lib/kyler/artifact-preview";
import { artifactSize, type Artifact } from "@/lib/kyler/artifacts";
import type { WorkspaceInput } from "@/lib/kyler/types";
import { WorkspaceTree } from "./workspace-tree";
import { PreviewRenderer } from "./preview-renderer";

type BrowserContext = {
  session: string;
  items: Artifact[];
  inputs: WorkspaceInput[];
  files: PreviewFile[];
  state: BrowserState;
  openFile: (key?: string) => void;
  close: () => void;
  resize: (width: number) => void;
  refreshing: boolean;
  error: string | null;
  reload: () => Promise<void>;
};
const Context = createContext<BrowserContext | null>(null);
export function useArtifactBrowser() {
  const context = useContext(Context);
  if (!context) throw new Error("Artifact preview requires a provider.");
  return context;
}
export function ArtifactsProvider({ session, children }: { session: string; children: ReactNode }) {
  const running = useAuiState((state) => state.thread.isRunning);
  const attachments = useAuiState((state) =>
    state.composer.attachments.map((item) => item.id).join(","),
  );
  const [items, setItems] = useState<Artifact[]>([]);
  const [inputs, setInputs] = useState<WorkspaceInput[]>([]);
  const [state, setState] = useState<BrowserState>(initialBrowserState);
  const currentState = useRef(state);
  currentState.current = state;
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const loadFiles = useRef<() => Promise<void>>(async () => undefined);
  const known = useRef<Set<string> | null>(null);
  const wasRunning = useRef(running);
  const files = useMemo(() => previewFiles(items, inputs), [items, inputs]);
  useEffect(() => {
    known.current = null;
    setItems([]);
    setInputs([]);
    setError(null);
    try {
      setState({
        ...restoreBrowserState(
          JSON.parse(localStorage.getItem(`kyler.preview.${session}`) ?? "null"),
        ),
        open: false,
        width: 70,
        dismissed: false,
      });
    } catch {
      setState(initialBrowserState);
    }
    setReady(true);
  }, [session]);
  useEffect(() => {
    if (ready) {
      try {
        localStorage.setItem(`kyler.preview.${session}`, JSON.stringify(state));
      } catch {
        /* Storage can be disabled. */
      }
    }
  }, [session, ready, state]);
  useEffect(() => {
    if (running && !wasRunning.current) setState((value) => ({ ...value, dismissed: false }));
    wasRunning.current = running;
  }, [running]);
  useEffect(() => {
    if (!ready) return;
    let controller: AbortController | undefined;
    const load = async () => {
      controller?.abort();
      const request = new AbortController();
      controller = request;
      await loadWorkspaceFiles(session, request.signal)
        .then((result) => {
          if (request.signal.aborted) return;
          setError(null);
          const next = previewFiles(result.outputs, result.inputs);
          if (!running) {
            const previous = known.current ?? new Set<string>();
            if (
              autoPreview(next, previous, session, currentState.current) !== currentState.current
            ) {
              setState((value) => autoPreview(next, previous, session, value));
            }
          }
          if (!running)
            known.current = new Set(next.filter((file) => file.available).map((file) => file.key));
          setItems((previous) =>
            JSON.stringify(previous) === JSON.stringify(result.outputs) ? previous : result.outputs,
          );
          setInputs((previous) =>
            JSON.stringify(previous) === JSON.stringify(result.inputs) ? previous : result.inputs,
          );
        })
        .catch((error) => {
          if (!request.signal.aborted) {
            setError("Unable to load session files. Check the backend connection and retry.");
            throw error;
          }
        });
    };
    loadFiles.current = load;
    void load().catch(() => undefined);
    const refresh = () => {
      if (document.visibilityState !== "hidden") void load().catch(() => undefined);
    };
    const timer = setInterval(refresh, 10000);
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      controller?.abort();
      clearInterval(timer);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [session, ready, running, attachments]);
  return (
    <Context.Provider
      value={{
        session,
        error,
        items,
        inputs,
        files,
        state,
        openFile: (key) => {
          setState((value) => ({
            ...value,
            open: true,
            width: value.open ? value.width : 70,
            dismissed: false,
            selected: key ?? value.selected ?? files.find((file) => !file.resource)?.key ?? null,
          }));
        },
        close: () => setState((value) => ({ ...value, open: false, dismissed: true })),
        resize: (width) =>
          setState((value) => ({ ...value, width: Math.max(30, Math.min(70, width)) })),
        refreshing,
        reload: async () => {
          setRefreshing(true);
          try {
            await loadFiles.current();
          } finally {
            setRefreshing(false);
          }
        },
      }}
    >
      {children}
    </Context.Provider>
  );
}
export function SessionFilesMenu() {
  const browser = useArtifactBrowser();
  const running = useAuiState((state) => state.thread.isRunning);
  return (
    <Button
      variant="ghost"
      size="icon-sm"
      title={browser.error ?? (running ? "Preview · Working" : "Preview")}
      className={`relative z-40 size-7 rounded-md border-0 text-muted-foreground/70 hover:bg-foreground/5 ${browser.state.open ? "bg-foreground/5 text-foreground" : "bg-transparent"}`}
      aria-expanded={browser.state.open}
      data-working={running}
      aria-label="Open file browser"
      onClick={() => (browser.state.open ? browser.close() : browser.openFile())}
    >
      <MonitorIcon
        className={`preview-monitor size-4 ${running ? "text-foreground" : ""}`}
        strokeWidth={running ? 2 : 1.5}
      />
      {browser.error && (
        <span className="absolute right-0 top-0 size-2 rounded-full bg-destructive" />
      )}
      {running && (
        <span role="status" className="sr-only">
          Working
        </span>
      )}
    </Button>
  );
}
export function ArtifactBrowserLayout({
  children,
  enabled,
}: {
  children: ReactNode;
  enabled: boolean;
}) {
  const browser = useArtifactBrowser();
  const container = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  const visible = enabled && browser.state.open;
  useEffect(() => {
    if (!visible) return;
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) {
        browser.close();
      }
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [visible, browser.close]);
  return (
    <div ref={container} className="relative flex h-full min-w-0 overflow-hidden">
      <div className="h-full min-w-0 flex-1">{children}</div>
      {visible && (
        <>
          <aside
            aria-label="File browser"
            style={{ "--preview-width": `${browser.state.width}%` } as React.CSSProperties}
            className="absolute inset-x-2 bottom-16 top-2 z-30 flex min-w-0 shrink-0 flex-col rounded-2xl border border-foreground/20 bg-background shadow-sm lg:relative lg:inset-auto lg:h-full lg:w-[var(--preview-width)]"
          >
            <div
              role="separator"
              aria-label="Resize file browser"
              aria-orientation="vertical"
              aria-valuenow={Math.round(browser.state.width)}
              aria-valuemin={30}
              aria-valuemax={70}
              tabIndex={0}
              title="Drag to resize · Double-click to reset"
              className="absolute inset-y-4 -left-1 z-40 hidden w-2 touch-none cursor-col-resize rounded-full outline-none hover:bg-foreground/10 focus-visible:bg-foreground/15 lg:block"
              onDoubleClick={() => browser.resize(70)}
              onPointerCancel={() => setDragging(false)}
              onKeyDown={(event) => {
                if (["ArrowLeft", "ArrowRight"].includes(event.key)) {
                  event.preventDefault();
                  browser.resize(
                    browser.state.width +
                      (event.key === "ArrowLeft" ? 1 : -1) * (event.shiftKey ? 10 : 2),
                  );
                } else if (event.key === "Home" || event.key === "End") {
                  event.preventDefault();
                  browser.resize(event.key === "Home" ? 30 : 70);
                }
              }}
              onPointerDown={(event) => {
                event.currentTarget.setPointerCapture(event.pointerId);
                setDragging(true);
              }}
              onPointerMove={(event) => {
                if (!dragging || !container.current) return;
                const rect = container.current.getBoundingClientRect();
                browser.resize(((rect.right - event.clientX) / rect.width) * 100);
              }}
              onPointerUp={() => setDragging(false)}
              onLostPointerCapture={() => setDragging(false)}
            />
            <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-2xl">
              <BrowserPanel />
            </div>
          </aside>
          {dragging && (
            <div className="fixed inset-0 z-50 touch-none select-none cursor-col-resize" />
          )}
        </>
      )}
    </div>
  );
}
function BrowserPanel() {
  const browser = useArtifactBrowser();
  const file = selectedPreviewFile(browser.files, browser.state.selected);
  const files = displayPreviewFiles(browser.files);
  const [treeOpen, setTreeOpen] = useState(false);
  const [history, setHistory] = useState<{ keys: string[]; index: number }>({
    keys: file ? [file.key] : [],
    index: 0,
  });
  useEffect(() => {
    if (!file) return;
    setHistory((previous) =>
      previous.keys[previous.index] === file.key
        ? previous
        : {
            keys: [...previous.keys.slice(0, previous.index + 1), file.key],
            index: previous.keys.length ? previous.index + 1 : 0,
          },
    );
  }, [file?.key]);
  const navigate = (offset: number) => {
    const index = history.index + offset;
    const key = history.keys[index];
    if (key && files.some((file) => file.key === key)) {
      setHistory((previous) => ({ ...previous, index }));
      browser.openFile(key);
    }
  };
  const [refreshError, setRefreshError] = useState(false);
  const previousKey = useRef(file?.key);
  previousKey.current = file?.key;
  const refreshFile = async () => {
    setRefreshError(false);
    const key = file?.key;
    try {
      await browser.reload();
      if (previousKey.current === key) setRefresh((value) => value + 1);
    } catch {
      if (previousKey.current === key) setRefreshError(true);
    }
  };
  const [source, setSource] = useState(false);
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    setSource(false);
    setRefreshError(false);
  }, [file?.key]);
  const textual =
    file &&
    ![
      "image",
      "pdf",
      "spreadsheet",
      "document",
      "slides",
      "audio",
      "video",
      "unsupported",
    ].includes(file.kind);
  return (
    <>
      <div
        onKeyDown={(event) => {
          if (event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
            event.preventDefault();
            navigate(event.key === "ArrowLeft" ? -1 : 1);
          }
        }}
        className="flex h-12 shrink-0 items-center gap-1 border-b border-border/60 bg-muted/15 px-2"
      >
        <Button
          variant="ghost"
          size="icon-xs"
          className="sm:hidden"
          aria-label="Toggle file tree"
          aria-expanded={treeOpen}
          onClick={() => setTreeOpen(!treeOpen)}
        >
          <FolderTreeIcon />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Previous file"
          title="Previous file"
          disabled={history.index <= 0}
          onClick={() => navigate(-1)}
        >
          <ChevronLeftIcon />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Next file"
          title="Next file"
          disabled={history.index >= history.keys.length - 1}
          onClick={() => navigate(1)}
        >
          <ChevronRightIcon />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Refresh preview"
          title="Refresh preview"
          disabled={browser.refreshing}
          onClick={() => void refreshFile()}
        >
          <RefreshCwIcon className={browser.refreshing ? "animate-spin" : ""} />
        </Button>
        <div className="min-w-0 flex-1 rounded-lg border border-border/50 bg-muted/40 px-3 py-1.5">
          <h2
            className="truncate text-xs text-muted-foreground"
            title={file ? previewPath(file) : undefined}
          >
            {file
              ? `${file.group === "generated" ? "outputs" : "inputs"} / ${previewPath(file)}`
              : "Session files"}
          </h2>
        </div>
        {file?.available && (
          <a
            href={file.href}
            download={file.name}
            aria-label={`Download ${file.name}`}
            title={`Download ${file.name}`}
            className="rounded-md p-1.5 text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
          >
            <DownloadIcon className="size-4" />
          </a>
        )}
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Close preview"
          title="Close preview"
          onClick={browser.close}
        >
          <XIcon />
        </Button>
      </div>
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <div className={`${treeOpen ? "flex" : "hidden"} min-h-0 sm:flex`}>
          <WorkspaceTree />
        </div>
        <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
          <div className="min-h-0 flex-1 overflow-hidden">
            {file ? (
              <PreviewRenderer
                key={file.key}
                file={file}
                files={browser.files}
                source={source}
                scripts={file.kind === "html"}
                refresh={refresh}
                onRetry={() => void refreshFile()}
              />
            ) : (
              <div className="grid h-full place-content-center p-8 text-sm text-muted-foreground">
                Generated and uploaded files appear here.
              </div>
            )}
          </div>
        </div>
      </div>
      <div className="flex h-9 shrink-0 items-center gap-2 border-t border-border/60 bg-muted/15 px-3">
        <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {refreshError
            ? "Refresh failed. Try again."
            : file
              ? `${file.name.split(".").at(-1)?.toUpperCase() ?? "File"} · ${artifactSize(file.bytes)}`
              : "Session files"}
        </span>
        {textual && (
          <Button
            variant="ghost"
            size="sm"
            aria-pressed={source}
            className="text-xs"
            onClick={() => setSource(!source)}
          >
            {source ? "Preview" : "Source"}
          </Button>
        )}
      </div>
    </>
  );
}
