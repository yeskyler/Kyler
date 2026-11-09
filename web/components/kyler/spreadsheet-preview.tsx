"use client";

import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { validateWorkbook } from "@/lib/kyler/spreadsheet";
import { spreadsheetAssets, spreadsheetFrameDocument } from "@/lib/kyler/spreadsheet-frame";

export function SpreadsheetPreview({ bytes }: { bytes: Uint8Array }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const [html, setHtml] = useState("");
  const [error, setError] = useState("");
  const [ready, setReady] = useState(false);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    let sent = false;
    const controller = new AbortController();
    const channel = crypto.randomUUID();
    setHtml("");
    setError("");
    setReady(false);
    const timer = setTimeout(
      () => fail("Workbook preview timed out. Retry or download the original file."),
      20000,
    );
    const fail = (message: string) => {
      if (!active) return;
      clearTimeout(timer);
      controller.abort();
      setError(message);
      setHtml("");
    };
    const receive = (event: MessageEvent) => {
      if (
        !active ||
        event.source !== frame.current?.contentWindow ||
        event.origin !== "null" ||
        event.data?.type !== "kyler-spreadsheet" ||
        event.data.channel !== channel
      )
        return;
      if (event.data.status === "ready" && !sent) {
        sent = true;
        const buffer = bytes.slice().buffer;
        frame.current?.contentWindow?.postMessage(
          { type: "kyler-spreadsheet-open", channel, buffer },
          "*",
          [buffer],
        );
      } else if (event.data.status === "rendered") {
        clearTimeout(timer);
        setReady(true);
      } else if (event.data.status === "error") {
        fail("This workbook could not be previewed. Retry or download it to open locally.");
      }
    };
    window.addEventListener("message", receive);
    void (async () => {
      validateWorkbook(bytes);
      const assets = await Promise.all(
        spreadsheetAssets.map(async (url) => {
          const response = await fetch(url, {
            signal: controller.signal,
            cache: "force-cache",
            redirect: "error",
          });
          if (!response.ok) throw new Error("Preview resources could not load.");
          const data = await response.text();
          if (!active || controller.signal.aborted) throw new Error("Preview cancelled.");
          return data;
        }),
      );
      if (active) setHtml(spreadsheetFrameDocument(channel, assets.slice(0, 3), assets[3]));
    })().catch((failure: Error) => {
      if (!controller.signal.aborted) fail(failure.message || "Workbook preview could not load.");
    });
    return () => {
      active = false;
      clearTimeout(timer);
      controller.abort();
      window.removeEventListener("message", receive);
    };
  }, [bytes, attempt]);
  return (
    <div ref={root} className="relative flex h-full min-h-0 flex-col bg-white text-neutral-900">
      <div className="flex shrink-0 items-center justify-between border-b border-neutral-200 px-3 py-1 text-xs">
        <span>Read-only workbook</span>
        <Button
          variant="ghost"
          size="xs"
          onClick={() => void root.current?.requestFullscreen?.().catch(() => undefined)}
        >
          Fullscreen
        </Button>
      </div>
      {error ? (
        <div
          role="alert"
          className="grid flex-1 place-content-center justify-items-center gap-3 p-5 text-center text-sm"
        >
          <p>{error}</p>
          <Button variant="outline" size="sm" onClick={() => setAttempt((value) => value + 1)}>
            Retry
          </Button>
        </div>
      ) : (
        <div className="relative min-h-0 flex-1">
          {!ready && (
            <div
              role="status"
              className="absolute inset-0 z-10 grid place-content-center bg-white text-sm"
            >
              Loading workbook…
            </div>
          )}
          {html && (
            <iframe
              ref={frame}
              title="Excel workbook"
              srcDoc={html}
              sandbox="allow-scripts"
              referrerPolicy="no-referrer"
              className="h-full w-full border-0"
            />
          )}
        </div>
      )}
    </div>
  );
}
