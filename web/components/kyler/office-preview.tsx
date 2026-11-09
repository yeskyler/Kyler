"use client";
import { useEffect, useState } from "react";
import { unzipSync } from "fflate";
import { validateWorkbook } from "@/lib/kyler/spreadsheet";

type Block = { text: string; heading: boolean; image?: string };
type Page = { name: string; blocks: Block[] };
const text = (element: Element) =>
  [...element.getElementsByTagNameNS("*", "t")].map((node) => node.textContent ?? "").join("");
export function OfficePreview({ bytes }: { bytes: Uint8Array }) {
  const [pages, setPages] = useState<Page[]>([]);
  const [error, setError] = useState("");
  useEffect(() => {
    const urls: string[] = [];
    try {
      validateWorkbook(bytes);
      const archive = unzipSync(bytes);
      const xml = (path: string) => {
        if (!archive[path]) throw new Error("Missing document part");
        const source = new TextDecoder().decode(archive[path]);
        if (/<!DOCTYPE/i.test(source)) throw new Error("Unsupported document declaration");
        const parsed = new DOMParser().parseFromString(source, "application/xml");
        if (parsed.querySelector("parsererror")) throw new Error("Invalid document part");
        return parsed;
      };
      const names = ["word/document.xml"];
      const result = names.map((path) => {
        const document = xml(path);
        const blocks: Block[] = [];
        const paragraphs = [...document.getElementsByTagNameNS("*", "p")].slice(0, 1000);
        for (const paragraph of paragraphs) {
          const content = text(paragraph);
          if (content)
            blocks.push({
              text: content,
              heading: !!paragraph
                .getElementsByTagNameNS("*", "pStyle")[0]
                ?.getAttribute("w:val")
                ?.match(/Title|Heading/i),
            });
        }
        const directory = path.slice(0, path.lastIndexOf("/") + 1);
        const filename = path.split("/").at(-1);
        const relPath = `${directory}_rels/${filename}.rels`;
        if (archive[relPath]) {
          const relationships = [...xml(relPath).getElementsByTagNameNS("*", "Relationship")];
          for (const relationship of relationships.slice(0, 50)) {
            const target = relationship.getAttribute("Target") ?? "";
            if (
              relationship.getAttribute("TargetMode") === "External" ||
              !/\.(png|jpe?g|gif|webp)$/i.test(target)
            )
              continue;
            const resolved = new URL(target, `https://office.invalid/${path}`);
            const image = archive[decodeURIComponent(resolved.pathname.slice(1))];
            if (!image || image.length > 5 * 1024 * 1024) continue;
            const url = URL.createObjectURL(
              new Blob([image], {
                type: `image/${target.toLowerCase().endsWith(".jpg") ? "jpeg" : target.split(".").at(-1)?.toLowerCase()}`,
              }),
            );
            urls.push(url);
            blocks.push({ text: "Embedded image", heading: false, image: url });
          }
        }
        return { name: "Document", blocks };
      });
      setPages(result);
    } catch {
      setError("This document could not be previewed. Download the original file.");
    }
    return () => urls.forEach((url) => URL.revokeObjectURL(url));
  }, [bytes]);
  if (error)
    return (
      <p role="alert" className="p-5 text-sm text-muted-foreground">
        {error}
      </p>
    );
  return (
    <div className="h-full overflow-auto bg-muted/20 p-4">
      <p className="mb-3 text-[11px] leading-5 text-muted-foreground">
        Content preview · Original layout, charts and advanced formatting may differ. Download for
        the complete document.
      </p>
      <div className="space-y-4">
        {pages.map((page, index) => (
          <section
            key={index}
            className="rounded-lg border border-border/60 bg-background p-6 shadow-sm"
          >
            <h3 className="mb-4 border-b border-border/60 pb-2 text-xs text-muted-foreground">
              {page.name}
            </h3>
            {page.blocks.map((block, i) =>
              block.image ? (
                <img
                  key={i}
                  src={block.image}
                  alt={block.text}
                  className="my-4 max-h-96 max-w-full object-contain"
                />
              ) : block.heading ? (
                <h4 key={i} className="my-4 text-lg font-semibold">
                  {block.text}
                </h4>
              ) : (
                <p key={i} className="my-2 whitespace-pre-wrap break-words text-sm leading-7">
                  {block.text}
                </p>
              ),
            )}
          </section>
        ))}
      </div>
    </div>
  );
}
