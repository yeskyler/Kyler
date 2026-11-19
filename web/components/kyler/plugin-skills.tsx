"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useAuiState } from "@assistant-ui/react";
import { Menu } from "@base-ui/react/menu";
import {
  BookOpenIcon,
  UploadIcon,
  LoaderCircleIcon,
  Trash2Icon,
  PowerIcon,
  SparklesIcon,
  ArrowRightIcon,
  FileArchiveIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { Sheet, SheetContent, SheetTitle, SheetDescription } from "@/components/ui/sheet";
import { api, KylerApiError } from "@/lib/kyler/client";
import {
  filterSkills,
  skillSources,
  draftFilesReviewed,
  type SkillDraft,
  type DraftPage,
  type DraftDetails,
} from "@/lib/kyler/plugins";
import type { Skill } from "@/lib/kyler/types";
import { useChatSettings } from "./chat-settings-context";
import {
  PluginActions,
  PluginEmpty,
  PluginNotice,
  PluginSkeletons,
  PluginToolbar,
  SourceBadge,
  StatusPill,
  pluginCard,
  pluginGrid,
  pluginIcon,
  pluginMenuItem,
  type PluginPanelProps,
} from "./plugin-ui";

export function PluginSkills({ active, addSignal, onCount }: PluginPanelProps) {
  const base = "plugins/skills";
  const { startSkillCreation, session } = useChatSettings();
  const running = useAuiState((state) => state.thread.isRunning);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [query, setQuery] = useState("");
  const [source, setSource] = useState("all");
  const [status, setStatus] = useState("all");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [busyId, setBusyId] = useState("");
  const [adding, setAdding] = useState(false);
  const [archive, setArchive] = useState<File>();
  const [review, setReview] = useState<{ info: Skill; instructions: string }>();
  const [removing, setRemoving] = useState<Skill>();
  const [removeError, setRemoveError] = useState("");
  const seenAdd = useRef(addSignal);
  const [highlight, setHighlight] = useState("");
  const [tab, setTab] = useState<"installed" | "drafts">("installed");
  const [drafts, setDrafts] = useState<SkillDraft[]>([]);
  const [draftReview, setDraftReview] = useState<DraftDetails>();
  const [file, setFile] = useState("");
  const [visited, setVisited] = useState<string[]>([]);
  const [confirmed, setConfirmed] = useState(false);
  const [draftLoading, setDraftLoading] = useState(false);
  const [cursor, setCursor] = useState<number | null>(null);
  const [draftTotal, setDraftTotal] = useState(0);
  const draftBase = `sessions/${session.session_id}/skills/drafts`;
  const generation = useRef(0);
  const loadDrafts = useCallback(
    async (offset = 0, signal?: AbortSignal) => {
      if (session.draft) {
        setDraftLoading(false);
        setDrafts([]);
        setCursor(null);
        setDraftTotal(0);
        return;
      }
      const current = generation.current;
      setDraftLoading(true);
      try {
        const page = await api<DraftPage>(`${draftBase}?cursor=${offset}&limit=20`, { signal });
        if (signal?.aborted || current !== generation.current) return;
        setDrafts((items) => (offset ? [...items, ...page.drafts] : page.drafts));
        setCursor(page.next_cursor);
        setDraftTotal(page.total);
      } finally {
        if (!signal?.aborted && current === generation.current) setDraftLoading(false);
      }
    },
    [draftBase, session.draft],
  );
  useEffect(() => {
    generation.current += 1;
    setDraftLoading(false);
    setDrafts([]);
    setDraftReview(undefined);
    setConfirmed(false);
    setCursor(null);
    setDraftTotal(0);
    setBusy(false);
    setError("");
    setNotice("");
  }, [draftBase]);
  useEffect(() => {
    if (!active || tab !== "drafts") return;
    const controller = new AbortController();
    void loadDrafts(0, controller.signal).catch((failure: Error) => {
      if (!controller.signal.aborted) setError(failure.message);
    });
    return () => controller.abort();
  }, [active, tab, loadDrafts]);
  async function inspectDraft(name: string) {
    if (busy) return;
    const current = generation.current;
    setBusy(true);
    setError("");
    setConfirmed(false);
    setDraftReview(undefined);
    try {
      const value = await api<DraftDetails>(`${draftBase}/${encodeURIComponent(name)}`);
      if (current !== generation.current) return;
      const first = Object.keys(value.draft.files)[0] ?? "";
      setDraftReview(value);
      setFile(first);
      setVisited(first ? [first] : []);
    } catch (failure) {
      if (current === generation.current) setError((failure as Error).message);
    } finally {
      if (current === generation.current) setBusy(false);
    }
  }
  async function publishDraft() {
    if (
      !draftReview ||
      busy ||
      running ||
      !confirmed ||
      !draftFilesReviewed(draftReview.draft.files, visited)
    )
      return;
    const current = generation.current;
    setBusy(true);
    setError("");
    try {
      await api(`${draftBase}/${encodeURIComponent(draftReview.draft.info.name)}/publish`, {
        method: "POST",
        body: JSON.stringify({
          expected_scope: draftReview.scope,
          fingerprint: draftReview.draft.info.fingerprint,
          expected_installed: draftReview.installed_fingerprint,
        }),
      });
      if (current !== generation.current) return;
      setDraftReview(undefined);
      setConfirmed(false);
      setNotice("Skill published. Existing enable/disable settings still apply.");
      const installed = await api<Skill[]>(base);
      if (current !== generation.current) return;
      setSkills(installed);
      await loadDrafts();
    } catch (failure) {
      if (current !== generation.current) return;
      if (failure instanceof KylerApiError && failure.status === 409) {
        setDraftReview(undefined);
        setConfirmed(false);
        setError("Content changed. Open the current draft and review every file again.");
        await loadDrafts().catch(() => {});
      } else setError((failure as Error).message);
    } finally {
      if (current === generation.current) setBusy(false);
    }
  }
  useEffect(() => {
    if (addSignal !== seenAdd.current && active) {
      setError("");
      setArchive(undefined);
      setAdding(true);
    }
    seenAdd.current = addSignal;
  }, [active, addSignal]);
  useEffect(() => {
    if (!loading) onCount(skills.length);
  }, [skills.length, loading, onCount]);
  useEffect(() => {
    if (!highlight || !active) return;
    const frame = requestAnimationFrame(() =>
      document
        .getElementById(`skill-${highlight}`)
        ?.scrollIntoView({ block: "nearest", behavior: "smooth" }),
    );
    return () => cancelAnimationFrame(frame);
  }, [highlight, active, skills]);
  useEffect(() => {
    let disposed = false;
    api<Skill[]>(base)
      .then((items) => {
        if (!disposed) setSkills(items);
      })
      .catch((failure: Error) => {
        if (!disposed) setError(failure.message);
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, []);
  async function install() {
    if (!archive || busy || running) return;
    setBusy(true);
    setBusyId("upload");
    setError("");
    setNotice("");
    try {
      if (!archive.name.toLowerCase().endsWith(".zip"))
        throw new Error("Choose a ZIP containing one skill and its SKILL.md.");
      if (archive.size > 20 * 1024 * 1024) throw new Error("Skill archive must be at most 20 MB.");
      const skill = await api<Skill>(`${base}/upload`, {
        method: "POST",
        headers: { "content-type": "application/zip" },
        body: archive,
      });
      setAdding(false);
      setArchive(undefined);
      setQuery("");
      setSource("all");
      setStatus("all");
      setNotice(`Installed ${skill.name}. Review its instructions before enabling it.`);
      setHighlight(skill.id);
      try {
        setSkills(await api<Skill[]>(base));
      } catch {
        setError("Skill installed, but the list could not refresh. Reopen Plugins to refresh it.");
      }
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function uninstall() {
    if (!removing || removing.source !== "user") return;
    setBusy(true);
    setBusyId(removing?.id ?? "");
    setRemoveError("");
    setError("");
    setNotice("");
    try {
      await api(`${base}/${removing.id}`, {
        method: "DELETE",
        body: JSON.stringify({ fingerprint: removing.fingerprint }),
      });
      setSkills((items) => items.filter((skill) => skill.id !== removing.id));
      setReview((current) => (current?.info.id === removing.id ? undefined : current));
      setRemoving(undefined);
      setNotice(`Uninstalled ${removing.name}.`);
      try {
        setSkills(await api<Skill[]>(base));
      } catch {
        setError(
          "Skill uninstalled, but the list could not refresh. Reload this page to see the latest skills.",
        );
      }
    } catch (error) {
      setRemoveError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function inspect(skill: Skill) {
    setBusy(true);
    setBusyId(skill.id);
    setError("");
    try {
      setReview(await api(`${base}/${skill.id}`));
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  async function toggle(skill: Skill) {
    setBusy(true);
    setBusyId(skill.id);
    setError("");
    try {
      await api(`${base}/${skill.id}/${skill.status === "enabled" ? "disable" : "enable"}`, {
        method: "POST",
        body: JSON.stringify({ fingerprint: skill.fingerprint }),
      });
      setSkills(await api(`${base}`));
      setReview(undefined);
    } catch (error) {
      setError((error as Error).message);
    } finally {
      setBusy(false);
    }
  }
  const filtered = filterSkills(skills, query, source, status);
  const clear = () => {
    setQuery("");
    setSource("all");
    setStatus("all");
  };
  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-center gap-2" aria-label="Skill library">
        <Button
          variant={tab === "installed" ? "default" : "outline"}
          disabled={busy}
          onClick={() => setTab("installed")}
        >
          Installed
        </Button>
        <Button
          variant={tab === "drafts" ? "default" : "outline"}
          disabled={busy}
          onClick={() => setTab("drafts")}
        >
          Drafts
        </Button>
      </div>
      {tab === "installed" && (
        <>
          <PluginToolbar
            kind="Skills"
            query={query}
            onQuery={setQuery}
            source={source}
            onSource={setSource}
            sources={skillSources}
            status={status}
            onStatus={setStatus}
            statuses={["enabled", "disabled", "needs_approval", "invalid", "shadowed"]}
          />
          <PluginNotice error={!review && !adding ? error : ""} notice={notice} />
          {loading ? (
            <PluginSkeletons />
          ) : (
            <>
              <p className="text-xs text-muted-foreground">
                {filtered.length === skills.length
                  ? `${skills.length} skills`
                  : `${filtered.length} of ${skills.length} skills`}
              </p>
              {!filtered.length ? (
                <PluginEmpty
                  filtered={!!query || source !== "all" || status !== "all"}
                  kind="skills"
                  onAdd={() => setAdding(true)}
                  onClear={clear}
                />
              ) : (
                <div className={pluginGrid}>
                  {filtered.map((skill) => (
                    <article
                      id={`skill-${skill.id}`}
                      key={skill.id}
                      className={`${pluginCard} ${highlight === skill.id ? "ring-2 ring-ring/20" : ""}`}
                    >
                      <div className="mb-4 flex items-start gap-3">
                        <div className={pluginIcon}>
                          <BookOpenIcon className="size-5" strokeWidth={1.5} />
                        </div>
                        <div className="min-w-0 flex-1 pt-0.5">
                          <h3>
                            <button
                              type="button"
                              disabled={busy}
                              className="max-w-full truncate text-left text-sm font-semibold outline-none hover:underline focus-visible:underline"
                              title={skill.name}
                              onClick={() => void inspect(skill)}
                            >
                              {skill.name}
                            </button>
                          </h3>
                          <div className="mt-1.5">
                            <SourceBadge>{skillSources[skill.source]}</SourceBadge>
                          </div>
                        </div>
                        <PluginActions label={skill.name} disabled={busy}>
                          <Menu.Item
                            className={pluginMenuItem}
                            disabled={busy}
                            onClick={() => void inspect(skill)}
                          >
                            View details
                          </Menu.Item>
                          {skill.status === "enabled" && (
                            <Menu.Item
                              className={pluginMenuItem}
                              disabled={busy || running}
                              onClick={() => void toggle(skill)}
                            >
                              <PowerIcon className="size-3.5" />
                              Disable
                            </Menu.Item>
                          )}
                          {skill.source === "user" && (
                            <>
                              <div className="my-1 border-t" role="separator" />
                              <Menu.Item
                                className={`${pluginMenuItem} text-destructive`}
                                disabled={busy || running || skill.status === "invalid"}
                                onClick={() => {
                                  setRemoveError("");
                                  setRemoving(skill);
                                }}
                              >
                                <Trash2Icon className="size-3.5" />
                                Uninstall
                              </Menu.Item>
                            </>
                          )}
                        </PluginActions>
                      </div>
                      <p className="mb-5 line-clamp-2 min-h-10 text-sm leading-5 text-muted-foreground">
                        {skill.description || "No description provided."}
                      </p>
                      {skill.error && (
                        <p
                          className="mb-4 line-clamp-2 text-xs text-destructive"
                          title={skill.error}
                        >
                          {skill.error}
                        </p>
                      )}
                      <div className="mt-auto flex flex-wrap items-center justify-between gap-2 border-t border-border/60 pt-4">
                        <StatusPill status={skill.status} />
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          disabled={busy}
                          onClick={() => void inspect(skill)}
                        >
                          {busy && busyId === skill.id ? (
                            <LoaderCircleIcon className="animate-spin" />
                          ) : null}
                          {!["enabled", "invalid", "shadowed"].includes(skill.status)
                            ? "Review & enable"
                            : "View details"}
                        </Button>
                      </div>
                    </article>
                  ))}
                </div>
              )}
            </>
          )}
        </>
      )}
      {tab === "drafts" && (
        <div className="space-y-5">
          <div className="flex items-center justify-between gap-3">
            <p className="text-xs text-muted-foreground">
              {draftLoading ? "Loading…" : `${draftTotal} drafts from this session’s memory scope`}
            </p>
            <Button
              variant="outline"
              disabled={busy || draftLoading}
              onClick={() => {
                setError("");
                void loadDrafts().catch((failure: Error) => setError(failure.message));
              }}
            >
              Refresh
            </Button>
          </div>
          <PluginNotice error={!draftReview ? error : ""} notice={notice} />
          {draftLoading && !drafts.length ? (
            <PluginSkeletons />
          ) : !drafts.length ? (
            <p className="rounded-xl border p-5 text-sm text-muted-foreground">
              {session.draft
                ? "Open or create a session to review its skill drafts."
                : "No skill drafts from this memory scope. Ask the agent to turn a reusable lesson into a draft."}
            </p>
          ) : (
            <div className={pluginGrid}>
              {drafts.map((draft) => (
                <article key={draft.name} className={pluginCard}>
                  <h3 className="text-sm font-semibold">{draft.name}</h3>
                  <p className="my-4 text-sm text-muted-foreground">{draft.description}</p>
                  <Button
                    variant="outline"
                    disabled={busy}
                    onClick={() => void inspectDraft(draft.name)}
                  >
                    Review draft
                  </Button>
                </article>
              ))}
            </div>
          )}
          {cursor !== null && (
            <Button
              variant="outline"
              disabled={busy || draftLoading}
              onClick={() =>
                void loadDrafts(cursor).catch((failure: Error) => setError(failure.message))
              }
            >
              Load more
            </Button>
          )}
        </div>
      )}
      <Dialog
        open={adding}
        onOpenChange={(open) => {
          if (!busy) {
            setAdding(open);
            setError("");
            setArchive(undefined);
          }
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogTitle>Add a skill</DialogTitle>
          <DialogDescription>
            Install a reusable workflow or create one with your agent.
          </DialogDescription>
          <div className="rounded-xl border p-4">
            <div className="mb-3 flex gap-3">
              <div className={pluginIcon}>
                <UploadIcon className="size-5" />
              </div>
              <div>
                <h3 className="text-sm font-medium">Upload a skill</h3>
                <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                  Choose a ZIP with a SKILL.md file. Up to 20 MB.
                </p>
              </div>
            </div>
            <Input
              type="file"
              accept=".zip,application/zip"
              aria-label="Skill ZIP archive"
              disabled={busy}
              onChange={(event) => {
                setArchive(event.target.files?.[0]);
                setError("");
              }}
            />
            {archive && (
              <p className="mt-2 flex items-center gap-1.5 text-xs text-muted-foreground">
                <FileArchiveIcon className="size-3.5" />
                {archive.name}
              </p>
            )}
          </div>
          <button
            type="button"
            disabled={busy || running}
            className="flex items-center gap-3 rounded-xl border p-4 text-left outline-none transition-colors hover:bg-muted/40 focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-50"
            onClick={() => {
              setAdding(false);
              void startSkillCreation().catch((failure: Error) => setError(failure.message));
            }}
          >
            <div className={pluginIcon}>
              <SparklesIcon className="size-5" />
            </div>
            <span className="flex-1">
              <span className="block text-sm font-medium">Create with your agent</span>
              <span className="mt-1 block text-xs text-muted-foreground">
                Start a conversation to build a new skill.
              </span>
            </span>
            <ArrowRightIcon className="size-4 text-muted-foreground" />
          </button>
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
              onClick={() => setAdding(false)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              disabled={busy || running || !archive}
              onClick={() => void install()}
            >
              {busy && <LoaderCircleIcon className="animate-spin" />}
              {busy ? "Installing…" : "Install skill"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
      <Dialog
        open={!!removing}
        onOpenChange={(open) => {
          if (!open && !busy) setRemoving(undefined);
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogTitle>Uninstall {removing?.name}?</DialogTitle>
          <DialogDescription>
            Remove this skill from your library. You can install it again later.
          </DialogDescription>
          {removeError && (
            <p role="alert" className="text-sm text-destructive">
              {removeError}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => setRemoving(undefined)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={busy || running}
              onClick={() => void uninstall()}
            >
              {busy ? "Uninstalling…" : "Uninstall"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
      <Sheet
        open={!!review}
        onOpenChange={(open) => {
          if (!open && !busy) setReview(undefined);
        }}
      >
        <SheetContent className="gap-0 data-[side=right]:w-full data-[side=right]:sm:max-w-xl">
          <div className="shrink-0 border-b p-6 pr-12">
            <div className={`${pluginIcon} mb-4`}>
              <BookOpenIcon className="size-5" />
            </div>
            <SheetTitle className="break-words text-xl font-semibold">
              {review?.info.name}
            </SheetTitle>
            <SheetDescription className="mt-2 leading-relaxed">
              {review?.info.description || "Skill details and instructions."}
            </SheetDescription>
            <div className="mt-4 flex items-center gap-3">
              {review && (
                <>
                  <SourceBadge>{skillSources[review.info.source]}</SourceBadge>
                  <StatusPill status={review.info.status} />
                </>
              )}
            </div>
          </div>
          <div className="min-h-0 flex-1 space-y-6 overflow-y-auto p-6">
            <div>
              <h3 className="mb-2 text-sm font-medium">Use in chat</h3>
              <code className="rounded-md bg-muted px-2 py-1 text-xs">@{review?.info.name}</code>
            </div>
            <div>
              <h3 className="text-sm font-medium">Instructions</h3>
              <p className="mb-3 mt-1 text-xs text-muted-foreground">
                Review what this skill asks your agent to do before enabling it.
              </p>
              <pre className="whitespace-pre-wrap break-words rounded-xl border bg-muted/30 p-4 font-mono text-xs leading-relaxed">
                {review?.instructions}
              </pre>
            </div>
            {!!review?.info.resources.length && (
              <details className="rounded-xl border p-4">
                <summary className="cursor-pointer text-sm font-medium">
                  Resource files{" "}
                  <span className="text-muted-foreground">({review.info.resources.length})</span>
                </summary>
                <ul className="mt-3 space-y-2 text-xs text-muted-foreground">
                  {review.info.resources.map((path) => (
                    <li key={path} className="break-all font-mono">
                      {path}
                    </li>
                  ))}
                </ul>
              </details>
            )}
            <details className="rounded-xl border p-4">
              <summary className="cursor-pointer text-sm font-medium">Installation details</summary>
              <p className="mt-3 break-all font-mono text-xs text-muted-foreground">
                {review?.info.path}
              </p>
            </details>
          </div>
          <div className="shrink-0 space-y-3 border-t p-6">
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {error}
              </p>
            )}
            <Button
              type="button"
              className="w-full"
              variant={review?.info.status === "enabled" ? "outline" : "default"}
              disabled={
                busy || running || !review || ["invalid", "shadowed"].includes(review.info.status)
              }
              onClick={() => review && void toggle(review.info)}
            >
              {busy && <LoaderCircleIcon className="animate-spin" />}
              {review?.info.status === "enabled" ? "Disable skill" : "Enable this version"}
            </Button>
          </div>
        </SheetContent>
      </Sheet>
      <Sheet
        open={!!draftReview}
        onOpenChange={(open) => {
          if (!open && !busy) {
            setDraftReview(undefined);
            setConfirmed(false);
          }
        }}
      >
        <SheetContent className="gap-0 data-[side=right]:w-full data-[side=right]:sm:max-w-2xl">
          <div className="shrink-0 border-b p-6 pr-12">
            <SheetTitle>{draftReview?.draft.info.name}</SheetTitle>
            <SheetDescription className="mt-2">
              Review every package file before publishing.
            </SheetDescription>
          </div>
          <div className="min-h-0 flex-1 space-y-5 overflow-y-auto p-6">
            <PluginNotice error={error} notice="" />
            {draftReview && (
              <>
                <p className="text-sm text-muted-foreground">
                  {draftReview.draft.info.description}
                </p>
                <p className="break-all text-xs text-muted-foreground">
                  Draft fingerprint: {draftReview.draft.info.fingerprint}
                </p>
                <p className="break-all text-xs text-muted-foreground">
                  {draftReview.installed_fingerprint
                    ? `Replaces installed fingerprint: ${draftReview.installed_fingerprint}`
                    : "Creates a new user skill"}
                </p>
                <p className="text-sm text-muted-foreground">
                  Publishing installs a user skill available across sessions, subject to existing
                  enable/disable settings.
                </p>
                <div
                  className="flex max-h-40 flex-wrap gap-2 overflow-auto"
                  aria-label="Draft files"
                >
                  {Object.keys(draftReview.draft.files).map((name) => (
                    <Button
                      key={name}
                      size="sm"
                      variant={file === name ? "default" : "outline"}
                      onClick={() => {
                        setFile(name);
                        setVisited((items) => (items.includes(name) ? items : [...items, name]));
                        setConfirmed(false);
                      }}
                    >
                      {visited.includes(name) ? "✓ " : ""}
                      {name}
                    </Button>
                  ))}
                </div>
                <pre className="max-h-[32rem] overflow-auto whitespace-pre-wrap break-words rounded-xl bg-muted/40 p-4 text-xs">
                  {draftReview.draft.files[file]}
                </pre>
                <p className="text-xs text-muted-foreground">
                  Opened {visited.length} of {Object.keys(draftReview.draft.files).length} files.
                  Files are shown as text; scripts are never run.
                </p>
                <label className="flex items-start gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={confirmed}
                    onChange={(event) => setConfirmed(event.target.checked)}
                    disabled={busy || !draftFilesReviewed(draftReview.draft.files, visited)}
                  />
                  I reviewed all files and the installed version being replaced.
                </label>
                <Button
                  disabled={
                    busy ||
                    running ||
                    !confirmed ||
                    !draftFilesReviewed(draftReview.draft.files, visited)
                  }
                  onClick={() => void publishDraft()}
                >
                  Publish reviewed draft
                </Button>
              </>
            )}
          </div>
        </SheetContent>
      </Sheet>
    </div>
  );
}
