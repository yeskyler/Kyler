"use client";

import { useEffect, useRef, useState } from "react";
import dynamic from "next/dynamic";
import { LoaderCircleIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { parseCsv, type PreviewFile } from "@/lib/kyler/artifact-preview";
import { prepareHtml, readPreview } from "@/lib/kyler/preview-content";
import { OfficePreview } from "./office-preview";
import { previewStorageKey, storageSnapshot } from "@/lib/kyler/preview-storage";
const SpreadsheetPreview = dynamic(
  () => import("./spreadsheet-preview").then((module) => module.SpreadsheetPreview),
  { ssr: false },
);
const PresentationPreview = dynamic(
  () => import("./presentation-preview").then((module) => module.PresentationPreview),
  { ssr: false },
);
const PdfPreview = dynamic(() => import("./pdf-preview").then((module) => module.PdfPreview), {
  ssr: false,
});
const MarkdownPreview = dynamic(
  () => import("./markdown-preview").then((module) => module.MarkdownPreview),
  { ssr: false },
);

export function PreviewRenderer({
  file,
  files,
  source,
  scripts,
  refresh,
  onRetry,
}: {
  file: PreviewFile;
  files: PreviewFile[];
  source: boolean;
  scripts: boolean;
  refresh: number;
  onRetry?: () => void;
}) {
  const [loaded, setLoaded] = useState<{
    bytes: Uint8Array;
    text: string;
    url: string;
    html: string;
    truncated: boolean;
    channel: string;
  }>();
  const [error, setError] = useState("");
  const iframe = useRef<HTMLIFrameElement>(null);
  const [zoom, setZoom] = useState(1);
  const current = useRef({ file, files });
  current.current = { file, files };
  useEffect(() => {
    const receive = (event: MessageEvent) => {
      if (
        event.source !== iframe.current?.contentWindow ||
        event.data?.type !== "kyler-preview-storage" ||
        event.data.channel !== loaded?.channel
      )
        return;
      const value = storageSnapshot(event.data.value);
      if (!value) return;
      try {
        localStorage.setItem(previewStorageKey(file.session, file.id), JSON.stringify(value));
      } catch {
        /* The in-frame store remains usable when persistence is unavailable. */
      }
    };
    window.addEventListener("message", receive);
    return () => window.removeEventListener("message", receive);
  }, [loaded?.channel, file.session, file.id]);
  useEffect(() => {
    const { file, files } = current.current;
    const controller = new AbortController();
    const urls: string[] = [];
    const objectUrl = (blob: Blob) => {
      const url = URL.createObjectURL(blob);
      urls.push(url);
      return url;
    };
    setLoaded(undefined);
    setError("");
    setZoom(1);
    if (!file.available || file.kind === "unsupported") return () => controller.abort();
    void readPreview(file, controller.signal)
      .then(async ({ bytes, truncated }) => {
        const text = [
          "image",
          "pdf",
          "spreadsheet",
          "document",
          "slides",
          "audio",
          "video",
        ].includes(file.kind)
          ? ""
          : new TextDecoder().decode(bytes);
        let storage: Record<string, string> = {};
        try {
          storage =
            storageSnapshot(
              JSON.parse(localStorage.getItem(previewStorageKey(file.session, file.id)) ?? "{}"),
            ) ?? {};
        } catch {
          /* Start with empty artifact storage. */
        }
        const channel = globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`;
        if (file.kind === "html" && truncated)
          throw new Error("HTML exceeds the 1 MiB preview limit. Download the original file.");
        const html =
          file.kind === "html"
            ? await prepareHtml(
                text,
                file,
                files,
                scripts,
                controller.signal,
                objectUrl,
                undefined,
                channel,
                storage,
              )
            : "";
        controller.signal.throwIfAborted();
        const extension = file.name.split(".").at(-1)?.toLowerCase() ?? "";
        const type =
          file.kind === "svg"
            ? "image/svg+xml"
            : file.kind === "image"
              ? `image/${extension === "jpg" ? "jpeg" : extension}`
              : file.kind === "audio"
                ? `audio/${extension === "m4a" ? "mp4" : extension}`
                : `video/${extension === "mov" ? "quicktime" : extension}`;
        const url = ["image", "svg", "audio", "video"].includes(file.kind)
          ? objectUrl(new Blob([bytes], { type }))
          : "";
        setLoaded({ bytes, text, url, html, truncated, channel });
      })
      .catch((failure: Error) => {
        if (!controller.signal.aborted) setError(failure.message || "Preview could not load.");
      });
    return () => {
      controller.abort();
      urls.forEach((url) => URL.revokeObjectURL(url));
    };
  }, [file.key, file.available, scripts, refresh]);
  if (!file.available)
    return <p className="p-6 text-sm text-muted-foreground">This retained file is unavailable.</p>;
  if (file.kind === "unsupported")
    return (
      <div className="grid h-full place-content-center gap-2 p-8 text-center">
        <p className="text-sm font-medium">Preview is not available for this file type.</p>
        <p className="text-xs text-muted-foreground">Use Download to open the original file.</p>
      </div>
    );
  if (error)
    return (
      <div
        role="alert"
        className="grid h-full place-content-center justify-items-center gap-3 p-8 text-center"
      >
        <p className="max-w-sm text-sm text-muted-foreground">{error}</p>
        {onRetry && (
          <Button variant="outline" size="sm" onClick={onRetry}>
            Try again
          </Button>
        )}
      </div>
    );
  if (!loaded)
    return (
      <div
        role="status"
        className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground"
      >
        <LoaderCircleIcon className="size-4 animate-spin" /> Loading preview…
      </div>
    );
  const textView = source || file.kind === "text";
  const csv =
    file.kind === "csv" && !source
      ? parseCsv(loaded.text, 200, file.name.toLowerCase().endsWith(".tsv") ? "\t" : ",")
      : null;
  return (
    <div className="flex h-full min-h-0 flex-col">
      {loaded.truncated && (
        <p className="shrink-0 border-b border-border/60 px-4 py-2 text-xs text-muted-foreground">
          Preview truncated to 1 MiB. Download the original for the complete file.
        </p>
      )}
      <div className="min-h-0 flex-1 overflow-auto">
        {textView ? (
          <pre className="whitespace-pre-wrap break-words p-5 font-mono text-xs leading-6">
            {file.name.toLowerCase().endsWith(".json") && !loaded.truncated
              ? prettyJson(loaded.text)
              : loaded.text}
          </pre>
        ) : file.kind === "html" ? (
          <iframe
            ref={iframe}
            key={`${file.key}:${refresh}:${scripts}`}
            title={`Preview ${file.name}`}
            srcDoc={loaded.html}
            sandbox={scripts ? "allow-scripts" : ""}
            referrerPolicy="no-referrer"
            allow="camera 'none'; microphone 'none'; geolocation 'none'; clipboard-read 'none'; clipboard-write 'none'"
            className="h-full w-full border-0 bg-white"
          />
        ) : ["image", "svg"].includes(file.kind) ? (
          <div className="flex h-full min-h-0 flex-col">
            <div className="flex justify-center gap-2 border-b border-border/60 p-2">
              <Button
                variant="ghost"
                size="xs"
                onClick={() => setZoom(Math.max(0.25, zoom - 0.25))}
              >
                −
              </Button>
              <Button variant="ghost" size="xs" onClick={() => setZoom(1)}>
                Fit
              </Button>
              <Button variant="ghost" size="xs" onClick={() => setZoom(Math.min(3, zoom + 0.25))}>
                +
              </Button>
            </div>
            <div className="grid min-h-0 flex-1 place-items-center overflow-auto bg-muted/20 p-4">
              <img
                src={loaded.url}
                alt={file.name}
                onError={() => setError("Image preview could not decode this file.")}
                className="max-h-full max-w-full object-contain"
                style={{ transform: `scale(${zoom})` }}
              />
            </div>
          </div>
        ) : file.kind === "audio" ? (
          <div className="grid h-full place-content-center gap-4 p-6">
            <p className="text-center text-sm text-muted-foreground">{file.name}</p>
            <audio controls preload="metadata" src={loaded.url} className="max-w-full" />
          </div>
        ) : file.kind === "video" ? (
          <div className="grid h-full place-items-center bg-black p-3">
            <video
              controls
              playsInline
              preload="metadata"
              src={loaded.url}
              className="max-h-full max-w-full"
            />
          </div>
        ) : file.kind === "slides" ? (
          <PresentationPreview bytes={loaded.bytes} />
        ) : file.kind === "document" ? (
          <OfficePreview bytes={loaded.bytes} />
        ) : file.kind === "spreadsheet" ? (
          <SpreadsheetPreview bytes={loaded.bytes} />
        ) : file.kind === "pdf" ? (
          <PdfPreview bytes={loaded.bytes} identity={file.key} />
        ) : csv ? (
          <div className="h-full overflow-auto">
            <table className="w-full border-collapse text-left text-xs">
              <thead className="sticky top-0 bg-muted">
                <tr>
                  {csv.rows[0]?.map((cell, i) => (
                    <th className="border-b border-r border-border px-3 py-2 font-medium" key={i}>
                      {cell || `Column ${i + 1}`}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {csv.rows.slice(1).map((row, i) => (
                  <tr key={i}>
                    {row.map((cell, j) => (
                      <td
                        className="max-w-80 whitespace-pre-wrap break-words border-b border-r border-border/50 px-3 py-2"
                        key={j}
                      >
                        {cell}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
            {csv.truncated && (
              <p className="p-3 text-xs text-muted-foreground">Showing the first 200 rows.</p>
            )}
          </div>
        ) : (
          <MarkdownPreview text={loaded.text} file={file} files={files} />
        )}
      </div>
    </div>
  );
}
function prettyJson(text: string) {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}
