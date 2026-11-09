"use client";

import { useEffect, useRef, useState } from "react";
import type { PDFDocumentProxy } from "pdfjs-dist";
import type { EventBus, PDFViewer } from "pdfjs-dist/web/pdf_viewer.mjs";
import "pdfjs-dist/web/pdf_viewer.css";
import { Button } from "@/components/ui/button";

const positions = new Map<string, number>();
export function PdfPreview({ bytes, identity }: { bytes: Uint8Array; identity: string }) {
  const root = useRef<HTMLDivElement>(null);
  const container = useRef<HTMLDivElement>(null);
  const pages = useRef<HTMLDivElement>(null);
  const controls = useRef<{ viewer: PDFViewer; bus: EventBus } | null>(null);
  const [document, setDocument] = useState<PDFDocumentProxy>();
  const [page, setPage] = useState(1);
  const [pageInput, setPageInput] = useState("1");
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState({ current: 0, total: 0 });
  const [thumbnails, setThumbnails] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    let loading: { destroy: () => Promise<void> } | undefined;
    let resize: ResizeObserver | undefined;
    let viewer: PDFViewer | undefined;
    setError("");
    setDocument(undefined);
    setQuery("");
    setMatches({ current: 0, total: 0 });
    void (async () => {
      // Viewer components expect the display layer to be initialized first.
      const pdfjs = await import("pdfjs-dist");
      const { EventBus, PDFViewer, PDFLinkService, PDFFindController } =
        await import("pdfjs-dist/web/pdf_viewer.mjs");
      if (!active || !container.current || !pages.current) return;
      pdfjs.GlobalWorkerOptions.workerSrc = new URL(
        "pdfjs-dist/build/pdf.worker.min.mjs",
        import.meta.url,
      ).toString();
      const bus = new EventBus();
      const links = new PDFLinkService({
        eventBus: bus,
        externalLinkTarget: 2,
        externalLinkRel: "noopener noreferrer",
      });
      const find = new PDFFindController({ eventBus: bus, linkService: links });
      viewer = new PDFViewer({
        container: container.current,
        viewer: pages.current,
        eventBus: bus,
        linkService: links,
        findController: find,
        textLayerMode: 1,
        imageResourcesPath: "/pdfjs/images/",
        annotationMode: 1,
      });
      links.setViewer(viewer);
      controls.current = { viewer, bus };
      bus.on("pagesinit", () => {
        if (!active || !viewer) return;
        viewer.currentScaleValue = "page-width";
        viewer.currentPageNumber = Math.min(viewer.pagesCount, positions.get(identity) ?? 1);
      });
      bus.on("pagechanging", ({ pageNumber }: { pageNumber: number }) => {
        if (!active) return;
        setPage(pageNumber);
        setPageInput(String(pageNumber));
        positions.set(identity, pageNumber);
        if (positions.size > 100) positions.delete(positions.keys().next().value!);
      });
      bus.on(
        "updatefindmatchescount",
        ({ matchesCount }: { matchesCount: { current: number; total: number } }) => {
          if (active) setMatches(matchesCount);
        },
      );
      bus.on(
        "updatefindcontrolstate",
        ({ matchesCount }: { matchesCount: { current: number; total: number } }) => {
          if (active && matchesCount) setMatches(matchesCount);
        },
      );
      const task = pdfjs.getDocument({
        data: bytes.slice(),
        cMapUrl: "/pdfjs/cmaps/",
        cMapPacked: true,
        standardFontDataUrl: "/pdfjs/standard_fonts/",
        wasmUrl: "/pdfjs/wasm/",
      });
      loading = task;
      const pdf = await task.promise;
      if (!active) return;
      viewer.setDocument(pdf);
      links.setDocument(pdf, null);
      setDocument(pdf);
      resize = new ResizeObserver(() => {
        const scale = viewer?.currentScaleValue;
        if (viewer && scale && ["page-width", "page-fit"].includes(scale))
          viewer.currentScaleValue = scale;
      });
      resize.observe(container.current);
    })().catch(() => {
      if (active) setError("PDF could not load. Retry or download the original file.");
    });
    return () => {
      active = false;
      resize?.disconnect();
      controls.current = null;
      viewer?.setDocument(null);
      void loading?.destroy().catch(() => undefined);
    };
  }, [bytes, identity]);
  const search = (value: string, again = false, previous = false) => {
    controls.current?.bus.dispatch("find", {
      source: root.current,
      type: again ? "again" : "",
      query: value,
      caseSensitive: false,
      entireWord: false,
      highlightAll: true,
      findPrevious: previous,
      matchDiacritics: false,
    });
  };
  const jump = (number: number) => {
    if (controls.current && document)
      controls.current.viewer.currentPageNumber = Math.max(1, Math.min(document.numPages, number));
  };
  return (
    <div ref={root} className="flex h-full min-h-0 flex-col bg-background">
      <div className="flex flex-wrap items-center gap-1 border-b border-border/60 p-2 text-xs">
        <Button
          size="xs"
          variant="ghost"
          aria-pressed={thumbnails}
          onClick={() => setThumbnails(!thumbnails)}
        >
          Pages
        </Button>
        <Button
          size="xs"
          variant="ghost"
          disabled={!document || page <= 1}
          onClick={() => jump(page - 1)}
          aria-label="Previous PDF page"
        >
          ←
        </Button>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            const value = Number(pageInput);
            if (Number.isInteger(value)) jump(value);
          }}
        >
          <input
            aria-label="PDF page number"
            className="w-10 rounded border px-1 text-center"
            inputMode="numeric"
            value={pageInput}
            onChange={(e) => setPageInput(e.target.value)}
          />{" "}
          / {document?.numPages ?? "…"}
        </form>
        <Button
          size="xs"
          variant="ghost"
          disabled={!document || page >= document.numPages}
          onClick={() => jump(page + 1)}
          aria-label="Next PDF page"
        >
          →
        </Button>
        <Button
          size="xs"
          variant="ghost"
          aria-label="Zoom out PDF"
          onClick={() => controls.current?.viewer.decreaseScale()}
        >
          −
        </Button>
        <Button
          size="xs"
          variant="ghost"
          aria-label="Zoom in PDF"
          onClick={() => controls.current?.viewer.increaseScale()}
        >
          +
        </Button>
        <Button
          size="xs"
          variant="ghost"
          onClick={() => {
            if (controls.current) controls.current.viewer.currentScaleValue = "page-width";
          }}
        >
          Width
        </Button>
        <Button
          size="xs"
          variant="ghost"
          onClick={() => {
            if (controls.current) controls.current.viewer.currentScaleValue = "page-fit";
          }}
        >
          Fit page
        </Button>
        <Button
          size="xs"
          variant="ghost"
          onClick={() => {
            if (controls.current)
              controls.current.viewer.pagesRotation =
                (controls.current.viewer.pagesRotation + 90) % 360;
          }}
        >
          Rotate
        </Button>
        <Button
          size="xs"
          variant="ghost"
          onClick={() => void root.current?.requestFullscreen?.().catch(() => undefined)}
        >
          Fullscreen
        </Button>
      </div>
      <form
        className="flex items-center gap-2 border-b border-border/60 p-2 text-xs"
        onSubmit={(e) => {
          e.preventDefault();
          search(query, true);
        }}
      >
        <input
          aria-label="Search PDF"
          placeholder="Search document"
          className="min-w-0 flex-1 rounded border bg-background px-2 py-1"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            search(e.target.value);
          }}
        />
        <span role="status">
          {matches.current} / {matches.total}
        </span>
        <Button
          size="xs"
          variant="ghost"
          type="button"
          aria-label="Previous search result"
          onClick={() => search(query, true, true)}
        >
          ↑
        </Button>
        <Button size="xs" variant="ghost" type="submit" aria-label="Next search result">
          ↓
        </Button>
      </form>
      {error && (
        <p role="alert" className="p-4 text-sm">
          {error}
        </p>
      )}
      {!document && !error && (
        <p role="status" className="p-4 text-sm">
          Loading PDF…
        </p>
      )}
      <div className="relative flex min-h-0 flex-1">
        {thumbnails && document && (
          <div className="w-28 shrink-0 overflow-auto border-r p-2">
            {Array.from({ length: document.numPages }, (_, index) => (
              <Thumbnail
                key={index}
                document={document}
                page={index + 1}
                selected={page === index + 1}
                onClick={() => jump(index + 1)}
              />
            ))}
          </div>
        )}
        <div className="relative min-w-0 flex-1">
          <div
            ref={container}
            className="absolute inset-0 overflow-auto bg-neutral-200"
            tabIndex={0}
            aria-label="PDF document"
          >
            <div ref={pages} className="pdfViewer" />
          </div>
        </div>
      </div>
    </div>
  );
}
function Thumbnail({
  document,
  page,
  selected,
  onClick,
}: {
  document: PDFDocumentProxy;
  page: number;
  selected: boolean;
  onClick: () => void;
}) {
  const button = useRef<HTMLButtonElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const observer = new IntersectionObserver(([entry]) => setVisible(entry.isIntersecting), {
      rootMargin: "100px",
    });
    if (button.current) observer.observe(button.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!visible || !canvas.current) return;
    let active = true;
    let task: { cancel: () => void } | undefined;
    const element = canvas.current;
    void document
      .getPage(page)
      .then(async (pdfPage) => {
        if (!active) return;
        const viewport = pdfPage.getViewport({
          scale: 84 / pdfPage.getViewport({ scale: 1 }).width,
        });
        element.width = Math.ceil(viewport.width);
        element.height = Math.ceil(viewport.height);
        task = pdfPage.render({ canvas: element, viewport });
        await (task as ReturnType<typeof pdfPage.render>).promise;
      })
      .catch(() => undefined);
    return () => {
      active = false;
      task?.cancel();
    };
  }, [document, page, visible]);
  return (
    <button
      ref={button}
      onClick={onClick}
      aria-label={`Go to PDF page ${page}`}
      aria-current={selected ? "page" : undefined}
      className={`mb-2 min-h-32 w-full rounded border p-1 text-xs ${selected ? "border-primary" : "border-transparent"}`}
    >
      {visible && <canvas ref={canvas} className="mx-auto max-w-full bg-white" />}
      <span>{page}</span>
    </button>
  );
}
