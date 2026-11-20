"use client";

import { useEffect, useState } from "react";
import { useAui, useAuiState } from "@assistant-ui/react";
import { Button } from "@/components/ui/button";
import { api, KylerApiError } from "@/lib/kyler/client";
import {
  draftFilesReviewed,
  matchesSkillProposal,
  type DraftDetails,
  type SkillDraft,
} from "@/lib/kyler/plugins";
import { useChatSettings } from "./chat-settings-context";

/** Kept outside the execution disclosure so pending decisions stay visible. */
export function SkillReview({ proposal, callId }: { proposal: SkillDraft; callId: string }) {
  const { session } = useChatSettings();
  const aui = useAui();
  const running = useAuiState((state) => state.thread.isRunning);
  const [details, setDetails] = useState<DraftDetails>();
  const [file, setFile] = useState("");
  const [visited, setVisited] = useState<string[]>([]);
  const [confirmed, setConfirmed] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [editing, setEditing] = useState(false);
  const [changes, setChanges] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const base = `sessions/${session.session_id}/skills/drafts/${encodeURIComponent(proposal.name)}`;
  useEffect(() => {
    const controller = new AbortController();
    setDetails(undefined);
    setVisited([]);
    setConfirmed(false);
    setFile("");
    setError("");
    api<DraftDetails>(base, { signal: controller.signal })
      .then((value) => {
        if (controller.signal.aborted) return;
        if (!matchesSkillProposal(value, proposal, session.session_id, callId)) {
          setError("This proposal has changed. Review the newer proposal in the conversation.");
          return;
        }
        setDetails(value);
      })
      .catch((failure: Error) => {
        if (!controller.signal.aborted)
          setError(
            failure instanceof KylerApiError && failure.status === 404
              ? "This proposal is no longer available in this session."
              : "Unable to load this skill proposal. Try again.",
          );
      });
    return () => controller.abort();
  }, [base, proposal.name, proposal.fingerprint, session.session_id, callId, attempt]);
  const state = details?.review_status ?? "pending";
  const pending = Boolean(details && state === "pending");
  const reviewed = Boolean(details && draftFilesReviewed(details.draft.files, visited));
  function openFile(name: string) {
    setFile(name);
    setVisited((items) => (items.includes(name) ? items : [...items, name]));
    setConfirmed(false);
  }
  async function decide(action: "publish" | "discard") {
    if (
      !details ||
      !pending ||
      busy ||
      running ||
      (action === "publish" && (!reviewed || !confirmed))
    )
      return;
    setBusy(true);
    setError("");
    try {
      await api(`${base}/${action}`, {
        method: "POST",
        body: JSON.stringify({
          expected_scope: details.scope,
          fingerprint: proposal.fingerprint,
          ...(action === "publish" ? { expected_installed: details.installed_fingerprint } : {}),
        }),
      });
      setDetails({ ...details, review_status: action === "publish" ? "installed" : "discarded" });
      setEditing(false);
      if (action === "publish") window.dispatchEvent(new Event("kyler-skills-changed"));
    } catch (failure) {
      if (failure instanceof KylerApiError && failure.status === 409) {
        setDetails(undefined);
        setVisited([]);
        setConfirmed(false);
        setError(
          "The proposal or installed skill changed. Reload and review the current files again.",
        );
      } else setError((failure as Error).message);
    } finally {
      setBusy(false);
    }
  }
  function requestChanges() {
    if (!details || !pending || !changes.trim() || busy || running) return;
    const source = JSON.parse(details.draft.files["references/learning-source.json"]);
    const sources = source.sources.map((item: { key: string; revision: number }) => ({
      key: item.key,
      revision: item.revision,
    }));
    const prompt = `Revise the proposed skill ${proposal.name} using memory draft_skill. Expected draft fingerprint: ${proposal.fingerprint}. Source memory revisions: ${JSON.stringify(sources)}. Keep it uninstalled and show the revised proposal for review. Requested changes:\n${changes.trim()}`;
    const previous = aui.composer().getState().text;
    aui.composer().setText([previous, prompt].filter(Boolean).join("\n\n"));
    setEditing(false);
    setChanges("");
  }
  return (
    <section
      className="my-4 space-y-3 rounded-xl border p-4 text-sm leading-6"
      aria-label={`Skill review: ${proposal.name}`}
    >
      <div>
        <p className="text-xs text-muted-foreground">
          {state === "installed"
            ? "Skill installed"
            : state === "discarded"
              ? "Proposal discarded"
              : "Skill ready for review"}
        </p>
        <h3 className="font-medium">{proposal.name}</h3>
        <p className="text-muted-foreground">{proposal.description}</p>
      </div>
      {!details && !error && <p role="status">Loading proposal…</p>}
      {state === "installed" && (
        <p className="text-muted-foreground">
          Available in Skills. Existing enable/disable settings still apply.
        </p>
      )}
      {state === "discarded" && (
        <p className="text-muted-foreground">This proposal will not be installed.</p>
      )}
      {details && (
        <>
          <Button
            variant="outline"
            size="sm"
            aria-expanded={expanded}
            onClick={() => {
              const next = !expanded;
              setExpanded(next);
              if (next && !file) openFile("SKILL.md");
            }}
          >
            {expanded ? "Hide files" : "Review files and sources"}
          </Button>
          {expanded && (
            <div className="space-y-3">
              <p className="text-xs text-muted-foreground">
                {details.installed_fingerprint && state === "pending"
                  ? "Replaces an installed skill. Review all files before installing."
                  : "Files are shown as text; scripts are never executed during review."}
              </p>
              <div className="flex flex-wrap gap-2" aria-label="Skill package files">
                {Object.keys(details.draft.files).map((name) => (
                  <Button
                    key={name}
                    size="sm"
                    variant={file === name ? "default" : "outline"}
                    onClick={() => openFile(name)}
                  >
                    {visited.includes(name) ? "✓ " : ""}
                    {name}
                  </Button>
                ))}
              </div>
              <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-muted/40 p-3 font-mono text-xs">
                {details.draft.files[file]}
              </pre>
              <p className="text-xs text-muted-foreground">
                Opened {visited.length} of {Object.keys(details.draft.files).length} files.
              </p>
            </div>
          )}
        </>
      )}
      {pending && (
        <>
          <label className="flex items-start gap-2 text-sm">
            <input
              type="checkbox"
              checked={confirmed}
              disabled={busy || !reviewed}
              onChange={(event) => setConfirmed(event.target.checked)}
            />
            I reviewed all files
            {details?.installed_fingerprint
              ? " and approve replacing the installed skill"
              : " and want to install this skill"}
            .
          </label>
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              disabled={busy || running || !reviewed || !confirmed}
              onClick={() => void decide("publish")}
            >
              {busy ? "Saving…" : "Install skill"}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy || running}
              onClick={() => setEditing(!editing)}
            >
              Request changes
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={busy || running}
              onClick={() => void decide("discard")}
            >
              Discard
            </Button>
          </div>
          {editing && (
            <div className="space-y-2">
              <textarea
                className="w-full rounded-lg border p-3 text-sm"
                aria-label="Requested skill changes"
                placeholder="What should change?"
                value={changes}
                onChange={(event) => setChanges(event.target.value)}
              />
              <Button
                size="sm"
                variant="outline"
                disabled={busy || running || !changes.trim()}
                onClick={requestChanges}
              >
                Add request to composer
              </Button>
              <p className="text-xs text-muted-foreground">
                Review the request in the message box, then send it to generate a new version.
              </p>
            </div>
          )}
        </>
      )}
      {error && (
        <div role="alert" className="space-y-2 text-destructive">
          <p>{error}</p>
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => setAttempt((n) => n + 1)}
          >
            Reload proposal
          </Button>
        </div>
      )}
    </section>
  );
}
