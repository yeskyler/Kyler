"use client";

import { type ComponentProps } from "react";
import { useArtifactBrowser } from "./artifact-browser";
import { useAuiState } from "@assistant-ui/react";
import {
  ArchiveIcon,
  DownloadIcon,
  FileIcon,
  FileSpreadsheetIcon,
  FileTextIcon,
  ImageIcon,
  PresentationIcon,
} from "lucide-react";
import { MarkdownText } from "@/components/assistant-ui/elements/markdown-text";
import { artifactReference, artifactSize, type Artifact } from "@/lib/kyler/artifacts";

export { ArtifactsProvider, ArtifactBrowserLayout, SessionFilesMenu } from "./artifact-browser";

function FileCard({
  href,
  label,
  info,
  compact = false,
}: {
  href: string;
  label: string;
  info: Pick<Artifact, "name" | "available" | "size_bytes" | "error">;
  compact?: boolean;
}) {
  const browser = useArtifactBrowser();
  const file = browser.files.find((item) => item.href === href);
  const extension = info.name.split(".").at(-1)?.toLowerCase();
  const Icon =
    extension === "pptx"
      ? PresentationIcon
      : extension === "xlsx" || extension === "csv"
        ? FileSpreadsheetIcon
        : ["png", "jpg", "jpeg", "webp"].includes(extension ?? "")
          ? ImageIcon
          : extension === "zip"
            ? ArchiveIcon
            : extension === "pdf" || extension === "docx"
              ? FileTextIcon
              : FileIcon;
  return (
    <span
      role="group"
      aria-label={label}
      className={`group flex w-full items-center gap-3 text-sm no-underline ${compact ? "rounded-xl px-3 py-2.5 hover:bg-muted/50" : "rounded-xl border border-border/80 bg-card px-4 py-3"}`}
    >
      <Icon aria-hidden className="size-6 shrink-0 text-muted-foreground" strokeWidth={1.5} />
      <span className="min-w-0 flex-1">
        <button
          type="button"
          disabled={!info.available || !file}
          onClick={() => file && browser.openFile(file.key)}
          title={info.name}
          className={`block font-medium text-foreground ${compact ? "truncate text-xs" : "break-words"}`}
        >
          {info.name}
        </button>
        <span
          title={info.error ?? undefined}
          className={`mt-0.5 block text-xs leading-5 text-muted-foreground ${compact ? "truncate" : ""}`}
        >
          {info.available
            ? `${extension?.toUpperCase() ?? "File"} · ${artifactSize(info.size_bytes)}`
            : info.error || "File unavailable. No retained file was found for this response."}
        </span>
      </span>
      {info.available && (
        <a
          href={href}
          download={info.name}
          className={`flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-xs font-medium text-foreground outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring ${compact ? "xl:opacity-0 xl:group-hover:opacity-100 xl:focus-visible:opacity-100" : ""}`}
          aria-label={`Download ${info.name}`}
        >
          <DownloadIcon aria-hidden className="size-4" />
          <span className={compact ? "sr-only" : undefined}>Download</span>
        </a>
      )}
    </span>
  );
}
function ArtifactCard({
  session,
  info,
  compact = false,
}: {
  session: string;
  info: Artifact;
  compact?: boolean;
}) {
  return (
    <FileCard
      label="Generated file"
      href={`/api/kyler/sessions/${session}/artifacts/${info.id}/content`}
      info={info}
      compact={compact}
    />
  );
}

function ArtifactLink({ href, children, ...props }: ComponentProps<"a">) {
  const browser = useArtifactBrowser();
  const execution = useAuiState((state) => state.message.metadata.custom.executionId);
  const reference = artifactReference(href);
  if (reference !== null) {
    const file = browser.files.find(
      (item) =>
        item.group === "generated" &&
        item.session === browser.session &&
        item.execution === execution &&
        artifactReference(item.reference) === reference,
    );
    return file?.available ? (
      <button
        type="button"
        onClick={() => browser.openFile(file.key)}
        className="text-primary underline underline-offset-2"
      >
        {children}
      </button>
    ) : (
      <span>{children}</span>
    );
  }
  return (
    <a
      {...props}
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      className="text-primary underline underline-offset-2 hover:text-primary/80"
    >
      {children}
    </a>
  );
}
function ArtifactImage({ src, alt, ...props }: ComponentProps<"img">) {
  if (typeof src === "string" && artifactReference(src) !== null)
    return <span>{alt || "Image"}</span>;
  return <img {...props} src={src} alt={alt ?? ""} />;
}
const components = { a: ArtifactLink, img: ArtifactImage };
export function ArtifactMarkdown() {
  return <MarkdownText components={components} />;
}

export function MessageArtifacts() {
  const { session, items } = useArtifactBrowser();
  const message = useAuiState((state) => state.message);
  if (message.status?.type === "running") return null;
  const files = items.filter(
    (item) =>
      !item.resource &&
      item.session_id === session &&
      item.execution_id === message.metadata.custom.executionId,
  );
  if (!files.length) return null;
  return (
    <section aria-label="Generated files" className="mt-5">
      <hr className="mb-4 border-0 border-t border-border/65" />
      <div className="space-y-2">
        {files.map((info) => (
          <ArtifactCard key={info.id} session={session} info={info} />
        ))}
      </div>
    </section>
  );
}
