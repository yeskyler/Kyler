import { installPreviewStorage, storageSnapshot } from "./preview-storage.ts";
import { parse, serialize, defaultTreeAdapter, html, type DefaultTreeAdapterTypes } from "parse5";
import { htmlPolicy, previewCdn, previewResource, type PreviewFile } from "./artifact-preview.ts";

const MAX_FILE = 20 * 1024 * 1024;
export const MAX_TEXT = 1024 * 1024;

export async function readPreview(file: PreviewFile, signal: AbortSignal) {
  const response = await fetch(file.href, { signal, cache: "no-store", redirect: "error" });
  if (!response.ok) throw new Error(`File could not load (${response.status}).`);
  const limit = ["image", "pdf", "spreadsheet", "document", "slides", "audio", "video"].includes(
    file.kind,
  )
    ? MAX_FILE
    : MAX_TEXT;
  const reader = response.body?.getReader();
  if (!reader) throw new Error("File content is unavailable.");
  const chunks: Uint8Array[] = [];
  let size = 0;
  let truncated = false;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      if (size + value.length > limit) {
        if (limit === MAX_FILE) throw new Error("The file exceeds the preview size limit.");
        chunks.push(value.subarray(0, limit - size));
        size = limit;
        truncated = true;
        await reader.cancel();
        break;
      }
      chunks.push(value);
      size += value.length;
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return { bytes, truncated };
}

/** Only retained, explicitly referenced sibling snapshots are available as HTML resources. */
export async function prepareHtml(
  source: string,
  owner: PreviewFile,
  files: PreviewFile[],
  scripts: boolean,
  signal: AbortSignal,
  objectUrl: (blob: Blob) => string,
  warning: (message: string) => void = () => undefined,
  channel = "",
  storage: Record<string, string> = {},
) {
  // Parse a plain syntax tree: opening a file must not trigger browser resource requests.
  const document = parse(source);
  type Element = DefaultTreeAdapterTypes.Element;
  const elements: Element[] = [];
  function walk(node: DefaultTreeAdapterTypes.Node) {
    if ("tagName" in node) elements.push(node);
    if ("childNodes" in node) node.childNodes.forEach(walk);
    if ("content" in node) walk(node.content);
  }
  walk(document);
  const attr = (node: Element, name: string) =>
    node.attrs.find((item) => item.name === name)?.value;
  const removeAttr = (node: Element, name: string) => {
    node.attrs = node.attrs.filter((item) => item.name !== name);
  };
  const setAttr = (node: Element, name: string, value: string) => {
    removeAttr(node, name);
    node.attrs.push({ name, value });
  };
  const remove = (node: Element) => defaultTreeAdapter.detachNode(node);
  for (const node of elements) {
    if (
      ["base", "iframe", "object", "embed"].includes(node.tagName) ||
      (node.tagName === "meta" && attr(node, "http-equiv") !== undefined)
    )
      remove(node);
    if (["a", "area"].includes(node.tagName)) {
      if (!attr(node, "href")?.startsWith("#")) removeAttr(node, "href");
      removeAttr(node, "target");
    }
    removeAttr(node, "srcset");
  }
  const loaded = new Map<string, Promise<string>>();
  const styles = new Map<string, string>();
  const dataFiles: Record<string, string> = {};
  let total = 0;
  async function asset(
    reference: string,
    base: PreviewFile,
    type: "image" | "style" | "script" | "font",
  ) {
    if (reference.startsWith("data:") && type === "image") return reference;
    const cdn = previewCdn(reference);
    if (cdn && ["style", "script"].includes(type)) return cdn;
    const file = previewResource(files, base, reference);
    if (!file) {
      warning(`Resource unavailable: ${reference.split(/[?#]/)[0].slice(0, 160)}`);
      return "";
    }
    if (type === "image" && !["image", "svg"].includes(file.kind)) return "";
    if (type === "script" && !/\.(m?js)$/i.test(file.name)) return "";
    if (type === "style" && !/\.css$/i.test(file.name)) return "";
    if (type === "font" && !/\.(woff2?|ttf|otf)$/i.test(file.name)) return "";
    if (loaded.has(file.key)) return loaded.get(file.key)!;
    if (loaded.size >= 24 || total >= MAX_FILE) return "";
    const request = (async () => {
      const { bytes, truncated } = await readPreview(file, signal);
      total += bytes.length;
      if (truncated || total > MAX_FILE) return "";
      let blob: Blob;
      let styleText = "";
      if (type === "style") {
        styleText = await css(new TextDecoder().decode(bytes), file);
        blob = new Blob([styleText], { type: "text/css" });
      } else
        blob = new Blob([bytes], {
          type:
            type === "script"
              ? "text/javascript"
              : type === "font"
                ? `font/${file.name.split(".").at(-1)?.toLowerCase()}`
                : `image/${file.name.split(".").at(-1)?.toLowerCase() === "svg" ? "svg+xml" : file.name.split(".").at(-1)?.toLowerCase() === "jpg" ? "jpeg" : file.name.split(".").at(-1)?.toLowerCase()}`,
        });
      if (type !== "style") {
        let binary = "";
        for (const value of bytes) binary += String.fromCharCode(value);
        return `data:${blob.type};base64,${btoa(binary)}`;
      }
      const url = objectUrl(blob);
      if (type === "style") styles.set(url, styleText);
      return url;
    })().catch(() => {
      signal.throwIfAborted();
      warning(`Resource could not load: ${reference.split(/[?#]/)[0].slice(0, 160)}`);
      return "";
    });
    loaded.set(file.key, request);
    return request;
  }
  async function css(text: string, base: PreviewFile) {
    // Imports are intentionally excluded; each linked stylesheet is an explicit retained resource.
    text = text.replace(/@import\s+(?:url\([^)]*\)|["'][^"']*["'])[^;]*;?/gi, "");
    const matches = [...text.matchAll(/url\(\s*(['"]?)(.*?)\1\s*\)/gi)];
    const urls = await Promise.all(
      matches.map((match) =>
        asset(match[2], base, /\.(woff2?|ttf|otf)(?:[?#]|$)/i.test(match[2]) ? "font" : "image"),
      ),
    );
    let index = 0;
    return text.replace(/url\(\s*(['"]?)(.*?)\1\s*\)/gi, () => `url("${urls[index++]}")`);
  }
  for (const node of elements) {
    if (["img", "source"].includes(node.tagName) && attr(node, "src") !== undefined)
      setAttr(node, "src", await asset(attr(node, "src") ?? "", owner, "image"));
    if (node.tagName === "video" && attr(node, "poster") !== undefined)
      setAttr(node, "poster", await asset(attr(node, "poster") ?? "", owner, "image"));
    if (node.tagName === "link") {
      if (attr(node, "rel")?.toLowerCase() !== "stylesheet") remove(node);
      else {
        removeAttr(node, "integrity");
        removeAttr(node, "crossorigin");
        const url = await asset(attr(node, "href") ?? "", owner, "style");
        if (styles.has(url)) {
          const style = defaultTreeAdapter.createElement("style", html.NS.HTML, []);
          defaultTreeAdapter.insertText(style, styles.get(url)!);
          if (node.parentNode) defaultTreeAdapter.insertBefore(node.parentNode, style, node);
          remove(node);
        } else if (url) {
          setAttr(node, "href", url);
          if (previewCdn(url)) setAttr(node, "crossorigin", "anonymous");
        } else remove(node);
      }
    }
    if (node.tagName === "script" && attr(node, "src") !== undefined) {
      removeAttr(node, "integrity");
      removeAttr(node, "crossorigin");
      const url = scripts ? await asset(attr(node, "src") ?? "", owner, "script") : "";
      if (previewCdn(url)) setAttr(node, "crossorigin", "anonymous");
      if (url) setAttr(node, "src", url);
      else remove(node);
    }
    if (node.tagName === "style") {
      const text = node.childNodes
        .filter((child) => child.nodeName === "#text")
        .map((child) => (child as DefaultTreeAdapterTypes.TextNode).value)
        .join("");
      node.childNodes = [];
      defaultTreeAdapter.insertText(node, await css(text, owner));
    }
    if (attr(node, "style") !== undefined)
      setAttr(node, "style", await css(attr(node, "style") ?? "", owner));
  }
  const head = elements.find((node) => node.tagName === "head")!;
  const policy = defaultTreeAdapter.createElement("meta", html.NS.HTML, [
    { name: "http-equiv", value: "Content-Security-Policy" },
    { name: "content", value: htmlPolicy(scripts) },
  ]);
  if (head.childNodes[0]) defaultTreeAdapter.insertBefore(head, policy, head.childNodes[0]);
  else defaultTreeAdapter.appendChild(head, policy);
  defaultTreeAdapter.appendChild(
    head,
    defaultTreeAdapter.createElement("meta", html.NS.HTML, [
      { name: "name", value: "referrer" },
      { name: "content", value: "no-referrer" },
    ]),
  );
  if (scripts && channel) {
    for (const file of files
      .filter(
        (file) =>
          file.resource &&
          file.available &&
          file.session === owner.session &&
          file.execution === owner.execution &&
          /\.(json|csv)$/i.test(file.name),
      )
      .slice(0, 24)) {
      let result;
      try {
        result = await readPreview(file, signal);
      } catch {
        signal.throwIfAborted();
        warning(`Data resource could not load: ${file.name}`);
        continue;
      }
      if (result.truncated || total + result.bytes.length > MAX_FILE) continue;
      total += result.bytes.length;
      const path = decodeURIComponent(new URL(file.reference, "https://preview.invalid/").pathname);
      dataFiles[path] = new TextDecoder().decode(result.bytes);
    }

    const bridge = defaultTreeAdapter.createElement("script", html.NS.HTML, []);
    const fileTable = JSON.stringify(dataFiles).replace(/</g, "\\u003c");
    const base = JSON.stringify(new URL(owner.reference, "https://preview.invalid/").href).replace(
      /</g,
      "\\u003c",
    );
    const storageBootstrap = `(${installPreviewStorage.toString()})(${JSON.stringify(storageSnapshot(storage) ?? {}).replace(/</g, "\\u003c")},${JSON.stringify(channel)});`;
    defaultTreeAdapter.insertText(
      bridge,
      `${storageBootstrap}(()=>{const files=${fileTable},base=${base},originalFetch=window.fetch.bind(window);window.fetch=(input,options)=>{try{const raw=typeof input==="string"?input:input.url;const url=new URL(raw,base);const path=decodeURIComponent(url.pathname);if(url.origin==="https://preview.invalid" && Object.prototype.hasOwnProperty.call(files,path) && (!options?.method || options.method.toUpperCase()==="GET"))return Promise.resolve(new Response(files[path],{headers:{"content-type":path.endsWith(".json")?"application/json":"text/csv"}}));}catch{}return originalFetch(input,options);};})();`,
    );
    const afterPolicy = head.childNodes[1];
    if (afterPolicy) defaultTreeAdapter.insertBefore(head, bridge, afterPolicy);
    else defaultTreeAdapter.appendChild(head, bridge);
  }
  signal.throwIfAborted();
  return serialize(document);
}
