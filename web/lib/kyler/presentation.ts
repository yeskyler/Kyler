import { unzipSync, zipSync, strFromU8, strToU8 } from "fflate";
import { validatePresentation } from "./spreadsheet.ts";

/** Remove dangling content-type declarations from a preview copy, never from the original. */
export function preparePresentation(bytes: Uint8Array) {
  const names = new Set(validatePresentation(bytes));
  try {
    const metadata = unzipSync(bytes, { filter: (file) => file.name === "[Content_Types].xml" })[
      "[Content_Types].xml"
    ];
    if (!metadata) throw new Error("Missing content types");
    if (metadata.length > 1024 * 1024) return bytes;
    const xml = strFromU8(metadata);
    const cleaned = xml.replace(/<(?:[A-Za-z_][\w.-]*:)?Override\b[^<>]*\/>/g, (tag) => {
      const path = /\bPartName\s*=\s*(["'])([^"']+)\1/.exec(tag)?.[2];
      if (!path || !/^\/?[A-Za-z0-9_./-]+$/.test(path) || path.includes("..")) return tag;
      return names.has(path.replace(/^\//, "")) ? tag : "";
    });
    if (cleaned === xml) return bytes;
    const entries = unzipSync(bytes);
    entries["[Content_Types].xml"] = strToU8(cleaned);
    return zipSync(entries);
  } catch {
    throw new Error("This presentation archive could not be read. Download the original file.");
  }
}
