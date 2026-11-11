"use client";

import { randomUuid } from "@/lib/kyler/random-uuid";

import { isValidElement, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import rehypeHighlight from "rehype-highlight";
import "katex/dist/katex.min.css";
import "highlight.js/styles/github-dark.css";
import "./markdown-preview.css";
import { previewResource, type PreviewFile } from "@/lib/kyler/artifact-preview";
import { Button } from "@/components/ui/button";

const positions = new Map<string, number>();
let diagramQueue = Promise.resolve();
function Diagram({ code }: { code: string }) {
  const [html, setHtml] = useState("");
  const [error, setError] = useState(false);
  useEffect(() => {
    let active = true;
    setHtml("");
    setError(false);
    diagramQueue = diagramQueue
      .catch(() => undefined)
      .then(async () => {
        if (!active) return;
        const { default: mermaid } = await import("mermaid");
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          maxTextSize: 50000,
          flowchart: { htmlLabels: false },
          theme: "default",
        });
        if (code.length > 50000) throw new Error("Diagram exceeds preview limit");
        await mermaid.parse(code);
        const { svg } = await mermaid.render(`diagram-${randomUuid()}`, code);
        if (active) setHtml(svg);
      })
      .catch(() => {
        if (active) setError(true);
      });
    return () => {
      active = false;
    };
  }, [code]);
  if (error)
    return (
      <div role="alert">
        <p>Diagram could not render.</p>
        <pre>{code}</pre>
      </div>
    );
  if (!html) return <p role="status">Rendering diagram…</p>;
  return (
    <iframe
      title="Mermaid diagram"
      sandbox=""
      referrerPolicy="no-referrer"
      className="h-96 w-full border-0 bg-white"
      srcDoc={`<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:"><style>body{margin:0;padding:12px}svg{display:block;margin:auto;max-width:100%;height:auto}</style>${html}`}
    />
  );
}
function CodeBlock({ children, className }: { children?: React.ReactNode; className?: string }) {
  const language = isValidElement<{ className?: string }>(children)
    ? (children.props.className?.match(/language-([\w+-]+)/)?.[1] ?? "text")
    : "text";
  const block = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return (
    <div ref={block} className={className}>
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span>{language}</span>
        <Button
          variant="ghost"
          size="xs"
          onClick={() => {
            void Promise.resolve()
              .then(() =>
                navigator.clipboard.writeText(
                  block.current?.querySelector("code")?.textContent ?? "",
                ),
              )
              .then(() => {
                setCopied(true);
                clearTimeout(timer.current);
                timer.current = setTimeout(() => setCopied(false), 1500);
              })
              .catch(() => setCopied(false));
          }}
        >
          {copied ? "Copied" : "Copy code"}
        </Button>
      </div>
      <pre>{children}</pre>
    </div>
  );
}
export function MarkdownPreview({
  text,
  file,
  files,
}: {
  text: string;
  file: PreviewFile;
  files: PreviewFile[];
}) {
  const root = useRef<HTMLDivElement>(null);
  const article = useRef<HTMLElement>(null);
  const [outline, setOutline] = useState<{ id: string; text: string; depth: number }[]>([]);
  const [font, setFont] = useState(15);
  const [image, setImage] = useState<{ src: string; alt: string } | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const headings = [...(article.current?.querySelectorAll("h1,h2,h3,h4,h5,h6") ?? [])];
    setOutline(
      headings.map((heading, index) => {
        heading.id = `markdown-heading-${index}`;
        return {
          id: heading.id,
          text: heading.textContent ?? "",
          depth: Number(heading.tagName[1]),
        };
      }),
    );
    if (root.current) root.current.scrollTop = positions.get(file.key) ?? 0;
    const element = root.current;
    return () => {
      if (element) positions.set(file.key, element.scrollTop);
      if (positions.size > 100) positions.delete(positions.keys().next().value!);
    };
  }, [text, file.key]);
  useEffect(() => {
    if (image && !dialog.current?.open) dialog.current?.showModal();
  }, [image]);
  return (
    <div ref={root} className="h-full overflow-auto bg-background">
      <div className="sticky top-0 z-10 flex items-center gap-2 border-b bg-background/95 px-4 py-2 text-xs">
        <details className="relative min-w-0 flex-1">
          <summary className="cursor-pointer">Contents ({outline.length})</summary>
          <nav
            aria-label="Markdown contents"
            className="absolute left-0 top-6 max-h-72 w-64 overflow-auto rounded border bg-background p-2 shadow-lg"
          >
            {outline.map((heading) => (
              <button
                key={heading.id}
                className="block w-full py-1 text-left hover:underline"
                style={{ paddingLeft: (heading.depth - 1) * 10 }}
                onClick={() => {
                  article.current
                    ?.querySelector(`#${heading.id}`)
                    ?.scrollIntoView({ block: "start" });
                  root.current?.querySelector("details")?.removeAttribute("open");
                }}
              >
                {heading.text}
              </button>
            ))}
          </nav>
        </details>
        <Button
          size="xs"
          variant="ghost"
          aria-label="Decrease Markdown font size"
          onClick={() => setFont(Math.max(12, font - 1))}
        >
          A−
        </Button>
        <span>{font}px</span>
        <Button
          size="xs"
          variant="ghost"
          aria-label="Increase Markdown font size"
          onClick={() => setFont(Math.min(24, font + 1))}
        >
          A+
        </Button>
        <Button
          size="xs"
          variant="ghost"
          onClick={() => void root.current?.requestFullscreen?.().catch(() => undefined)}
        >
          Fullscreen
        </Button>
      </div>
      <article
        ref={article}
        className="kyler-markdown mx-auto max-w-4xl px-6 py-5"
        style={{ fontSize: font }}
      >
        <ReactMarkdown
          remarkPlugins={[remarkGfm, remarkMath]}
          rehypePlugins={[
            [rehypeKatex, { throwOnError: false, strict: "ignore", trust: false }],
            [rehypeHighlight, { ignoreMissing: true }],
          ]}
          skipHtml
          components={{
            pre: ({ children }) => {
              if (
                isValidElement<{ className?: string; children?: React.ReactNode }>(children) &&
                children.props.className?.includes("language-mermaid")
              ) {
                return <Diagram code={String(children.props.children).replace(/\n$/, "")} />;
              }
              return <CodeBlock>{children}</CodeBlock>;
            },
            img: ({ src, alt }) => {
              const resource =
                typeof src === "string" ? previewResource(files, file, src) : undefined;
              if (!resource || !["image", "svg"].includes(resource.kind))
                return (
                  <span className="text-muted-foreground">
                    [Image unavailable: {alt || "image"}]
                  </span>
                );
              return (
                <img
                  src={resource.href}
                  alt={alt ?? ""}
                  loading="lazy"
                  className="cursor-zoom-in"
                  onClick={() => setImage({ src: resource.href, alt: alt ?? "Image" })}
                />
              );
            },
            a: ({ href, children }) => {
              if (href?.startsWith("#"))
                return (
                  <a
                    href={href}
                    onClick={(event) => {
                      event.preventDefault();
                      let fragment: string;
                      try {
                        fragment = decodeURIComponent(href.slice(1));
                      } catch {
                        return;
                      }
                      const normalized = (value: string) =>
                        value
                          .toLowerCase()
                          .trim()
                          .replace(/[^\p{L}\p{N}\s_-]/gu, "")
                          .replace(/\s/g, "-");
                      const heading = outline.find(
                        (item) => item.id === fragment || normalized(item.text) === fragment,
                      );
                      if (heading)
                        article.current
                          ?.querySelector(`#${heading.id}`)
                          ?.scrollIntoView({ block: "start" });
                    }}
                  >
                    {children}
                  </a>
                );
              const resource = href ? previewResource(files, file, href) : undefined;
              if (resource)
                return (
                  <a href={resource.href} download={resource.name}>
                    {children}
                  </a>
                );
              if (href && /^https?:\/\//i.test(href))
                return (
                  <a href={href} target="_blank" rel="noopener noreferrer">
                    {children}
                  </a>
                );
              return <span title="Linked file is not retained">{children}</span>;
            },
          }}
        >
          {text}
        </ReactMarkdown>
      </article>
      {image && (
        <dialog
          ref={dialog}
          className="m-auto max-h-[90vh] max-w-[95vw] rounded-lg bg-background p-4 backdrop:bg-black/70"
          onClose={() => setImage(null)}
        >
          <Button
            size="sm"
            variant="outline"
            className="mb-3"
            onClick={() => dialog.current?.close()}
          >
            Close image
          </Button>
          <img src={image.src} alt={image.alt} className="max-h-[75vh] max-w-full object-contain" />
        </dialog>
      )}
    </div>
  );
}
