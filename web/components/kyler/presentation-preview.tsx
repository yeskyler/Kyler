"use client";

import { randomUuid } from "@/lib/kyler/random-uuid";

import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { presentationAssets, presentationFrameDocument } from "@/lib/kyler/presentation-frame";

export function PresentationPreview({ bytes }: { bytes: Uint8Array }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const channel = useRef("");
  const [html, setHtml] = useState("");
  const [error, setError] = useState("");
  const [count, setCount] = useState(0);
  const [page, setPage] = useState(1);
  const [input, setInput] = useState("1");
  const [zoom, setZoom] = useState(1);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    let sent = false;
    let prepared: ArrayBuffer;
    let worker: Worker | undefined;
    const controller = new AbortController();
    const id = randomUuid();
    channel.current = id;
    setHtml("");
    setError("");
    setCount(0);
    setPage(1);
    setInput("1");
    setZoom(1);
    const fail = (message: string) => {
      if (!active) return;
      clearTimeout(timer);
      controller.abort();
      worker?.terminate();
      setError(message);
      setHtml("");
      setCount(0);
    };
    const timer = setTimeout(
      () => fail("Presentation preview timed out. Retry or download the original file."),
      30000,
    );
    const receive = (event: MessageEvent) => {
      if (
        !active ||
        event.source !== frame.current?.contentWindow ||
        event.origin !== "null" ||
        event.data?.channel !== id ||
        event.data?.type !== "kyler-presentation"
      )
        return;
      const data = event.data;
      if (data.status === "ready" && !sent) {
        sent = true;
        const buffer = prepared;
        frame.current?.contentWindow?.postMessage(
          { type: "kyler-presentation-open", channel: id, buffer },
          "*",
          [buffer],
        );
      } else if (
        (data.status === "rendered" || data.status === "position") &&
        Number.isInteger(data.count) &&
        data.count > 0 &&
        data.count <= 100 &&
        Number.isInteger(data.page) &&
        data.page > 0 &&
        data.page <= data.count
      ) {
        clearTimeout(timer);
        setCount(data.count);
        setPage(data.page);
        setInput(String(data.page));
        if (Number.isFinite(data.zoom) && data.zoom >= 0.1 && data.zoom <= 4) setZoom(data.zoom);
      } else if (data.status === "error")
        fail("This presentation could not be previewed. Retry or download it to open locally.");
    };
    window.addEventListener("message", receive);
    void (async () => {
      const preparation = new Promise<ArrayBuffer>((resolve, reject) => {
        worker = new Worker(new URL("../../lib/kyler/presentation-worker.ts", import.meta.url), {
          type: "module",
        });
        worker.onmessage = (event: MessageEvent<{ buffer?: ArrayBuffer; error?: string }>) => {
          worker?.terminate();
          if (event.data.buffer instanceof ArrayBuffer) resolve(event.data.buffer);
          else reject(new Error(event.data.error || "Presentation preparation failed."));
        };
        worker.onerror = () =>
          reject(
            new Error("Presentation preparation failed. Retry or download the original file."),
          );
        const buffer = bytes.slice().buffer;
        worker.postMessage(buffer, [buffer]);
      });
      const [scripts, buffer] = await Promise.all([
        Promise.all(
          presentationAssets.map(async (url) => {
            const response = await fetch(url, {
              signal: controller.signal,
              cache: "force-cache",
              redirect: "error",
            });
            if (!response.ok) throw new Error("Presentation preview resources could not load.");
            const source = await response.text();
            if (!active || controller.signal.aborted) throw new Error("Preview cancelled.");
            return source;
          }),
        ),
        preparation,
      ]);
      prepared = buffer;
      if (active) setHtml(presentationFrameDocument(id, scripts));
    })().catch((failure: Error) => {
      if (!controller.signal.aborted)
        fail(failure.message || "Presentation preview could not load.");
    });
    return () => {
      active = false;
      worker?.terminate();
      clearTimeout(timer);
      controller.abort();
      window.removeEventListener("message", receive);
    };
  }, [bytes, attempt]);
  const command = (detail: { page?: number; zoom?: number; fit?: boolean }) => {
    frame.current?.contentWindow?.postMessage(
      { type: "kyler-presentation-control", channel: channel.current, ...detail },
      "*",
    );
  };
  return (
    <div ref={root} className="relative flex h-full min-h-0 flex-col bg-background">
      <div className="flex shrink-0 flex-wrap items-center gap-1 border-b p-2 text-xs">
        <Button
          variant="ghost"
          size="xs"
          aria-label="Previous slide"
          disabled={!count || page <= 1}
          onClick={() => command({ page: page - 1 })}
        >
          ←
        </Button>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            const value = Number(input);
            if (Number.isInteger(value) && value >= 1 && value <= count) command({ page: value });
            else setInput(String(page));
          }}
        >
          <input
            aria-label="Slide number"
            disabled={!count}
            className="w-10 rounded border bg-background px-1 text-center"
            inputMode="numeric"
            value={input}
            onChange={(event) => setInput(event.target.value)}
          />{" "}
          / {count || "…"}
        </form>
        <Button
          variant="ghost"
          size="xs"
          aria-label="Next slide"
          disabled={!count || page >= count}
          onClick={() => command({ page: page + 1 })}
        >
          →
        </Button>
        <Button
          variant="ghost"
          size="xs"
          aria-label="Zoom out presentation"
          disabled={!count || zoom <= 0.1}
          onClick={() => command({ zoom: Math.max(0.1, zoom - 0.25) })}
        >
          −
        </Button>
        <span aria-label="Presentation zoom">{Math.round(zoom * 100)}%</span>
        <Button
          variant="ghost"
          size="xs"
          aria-label="Zoom in presentation"
          disabled={!count || zoom >= 4}
          onClick={() => command({ zoom: Math.min(4, zoom + 0.25) })}
        >
          +
        </Button>
        <Button variant="ghost" size="xs" disabled={!count} onClick={() => command({ fit: true })}>
          Fit
        </Button>
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
          {!count && (
            <div
              role="status"
              className="absolute inset-0 z-10 grid place-content-center bg-background text-sm"
            >
              Loading presentation…
            </div>
          )}
          {html && (
            <iframe
              ref={frame}
              title="PPTX presentation"
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
