import type { Artifact } from "./artifacts.ts";
import type { WorkspaceInput } from "./types.ts";

export type PreviewKind =
  | "html"
  | "pdf"
  | "image"
  | "markdown"
  | "csv"
  | "text"
  | "spreadsheet"
  | "document"
  | "slides"
  | "audio"
  | "video"
  | "svg"
  | "unsupported";
export type PreviewFile = {
  key: string;
  id: string;
  session: string;
  name: string;
  href: string;
  reference: string;
  group: "generated" | "uploaded";
  execution: string | null;
  bytes: number;
  available: boolean;
  kind: PreviewKind;
  resource?: boolean;
  primary?: boolean;
};
export type BrowserState = {
  selected: string | null;
  open: boolean;
  width: number;
  dismissed: boolean;
};
export const initialBrowserState: BrowserState = {
  selected: null,
  open: false,
  width: 70,
  dismissed: false,
};

export function previewKind(name: string): PreviewKind {
  const extension = name.split(".").at(-1)?.toLowerCase() ?? "";
  if (["html", "htm"].includes(extension)) return "html";
  if (extension === "docx") return "document";
  if (extension === "pptx") return "slides";
  if (["mp3", "wav", "ogg", "m4a", "flac"].includes(extension)) return "audio";
  if (["mp4", "webm", "ogv", "mov"].includes(extension)) return "video";
  if (extension === "svg") return "svg";
  if (extension === "pdf") return "pdf";
  if (["png", "jpg", "jpeg", "webp", "gif", "avif"].includes(extension)) return "image";
  if (["md", "markdown"].includes(extension)) return "markdown";
  if (["csv", "tsv"].includes(extension)) return "csv";
  if (extension === "xlsx") return "spreadsheet";
  if (
    [
      "txt",
      "json",
      "jsonl",
      "js",
      "mjs",
      "jsx",
      "ts",
      "tsx",
      "css",
      "scss",
      "rs",
      "py",
      "sh",
      "bash",
      "zsh",
      "toml",
      "yaml",
      "yml",
      "xml",
      "svg",
      "sql",
      "log",
      "ini",
      "conf",
    ].includes(extension)
  )
    return "text";
  return "unsupported";
}
export function previewFiles(outputs: Artifact[], inputs: WorkspaceInput[]): PreviewFile[] {
  return [
    ...outputs.map((file) => ({
      key: `generated:${file.session_id}:${file.id}`,
      id: file.id,
      session: file.session_id,
      name: file.name,
      href: `/api/kyler/sessions/${file.session_id}/artifacts/${file.id}/content`,
      reference: file.reference,
      group: "generated" as const,
      execution: file.execution_id,
      bytes: file.size_bytes,
      available: file.available,
      kind: previewKind(file.name),
      resource: file.resource,
      primary: file.primary,
    })),
    ...inputs.map(({ session_id, attachment: file }) => ({
      key: `uploaded:${session_id}:${file.id}`,
      id: file.id,
      session: session_id,
      name: file.name,
      href: `/api/kyler/sessions/${session_id}/attachments/${file.id}/content`,
      reference: file.name,
      group: "uploaded" as const,
      execution: null,
      bytes: file.bytes,
      available: true,
      kind: previewKind(file.name),
    })),
  ];
}

/** Open new retained outputs, preferring a primary or previewable deliverable. */
export function autoPreview(
  files: PreviewFile[],
  known: Set<string>,
  session: string,
  state: BrowserState,
) {
  const candidates = files.filter(
    (file) =>
      !known.has(file.key) &&
      file.group === "generated" &&
      file.session === session &&
      file.available &&
      !file.resource,
  );
  const candidate =
    candidates.find((file) => file.primary === true && file.kind !== "unsupported") ??
    candidates.find((file) => file.kind !== "unsupported") ??
    candidates[0];
  return candidate && !state.open && !state.dismissed
    ? { ...state, selected: candidate.key, open: true, width: 70 }
    : state;
}
export function restoreBrowserState(value: unknown): BrowserState {
  if (!value || typeof value !== "object") return { ...initialBrowserState };
  const data = value as Partial<BrowserState>;
  return {
    selected: typeof data.selected === "string" ? data.selected : null,
    open: data.open === true,
    width:
      typeof data.width === "number" && Number.isFinite(data.width)
        ? Math.min(70, Math.max(30, data.width))
        : 70,
    dismissed: data.dismissed === true,
  };
}

export function parseCsv(
  source: string,
  limit = 200,
  delimiter = ",",
): { rows: string[][]; truncated: boolean } {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  for (let i = 0; i < source.length; i++) {
    const ch = source[i];
    if (ch === '"' && (quoted || cell === "")) {
      if (quoted && source[i + 1] === '"') {
        cell += '"';
        i++;
      } else quoted = !quoted;
    } else if (ch === delimiter && !quoted) {
      row.push(cell);
      cell = "";
    } else if ((ch === "\n" || ch === "\r") && !quoted) {
      if (ch === "\r" && source[i + 1] === "\n") i++;
      row.push(cell);
      rows.push(row);
      row = [];
      cell = "";
      if (rows.length >= limit) return { rows, truncated: i < source.length - 1 };
    } else cell += ch;
  }
  if (cell || row.length) {
    row.push(cell);
    rows.push(row);
  }
  return { rows, truncated: false };
}

/** Resolve only relative references to retained snapshots from the same execution. */
export function previewResource(files: PreviewFile[], owner: PreviewFile, reference: string) {
  if (!reference || /^(?:[a-z][a-z\d+.-]*:|\/\/|#|\/)/i.test(reference)) return undefined;
  const normalize = (value: string, base = "https://preview.invalid/") => {
    try {
      return decodeURIComponent(new URL(value, base).pathname);
    } catch {
      return null;
    }
  };
  let base: string;
  try {
    base = new URL(owner.reference, "https://preview.invalid/").href;
  } catch {
    return undefined;
  }
  const path = normalize(reference, base);
  return files.find(
    (file) =>
      path !== null &&
      file.available &&
      file.group === "generated" &&
      file.session === owner.session &&
      file.execution === owner.execution &&
      normalize(file.reference) === path,
  );
}

export const PREVIEW_CDNS = [
  "https://cdn.jsdelivr.net",
  "https://cdnjs.cloudflare.com",
  "https://unpkg.com",
  "https://esm.sh",
];
export function previewCdn(reference: string) {
  try {
    const url = new URL(reference);
    return PREVIEW_CDNS.includes(url.origin) && !url.username && !url.password ? url.href : null;
  } catch {
    return null;
  }
}
export const HTML_POLICY =
  "default-src 'none'; script-src 'none'; style-src 'unsafe-inline' blob:; img-src data: blob:; font-src data: blob:; media-src data: blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";
export function htmlPolicy(scripts: boolean) {
  const policy = HTML_POLICY.replace(
    "style-src 'unsafe-inline' blob:",
    `style-src 'unsafe-inline' blob: ${PREVIEW_CDNS.join(" ")}`,
  );
  return scripts
    ? policy.replace(
        "script-src 'none'",
        `script-src 'unsafe-inline' blob: data: ${PREVIEW_CDNS.join(" ")}`,
      )
    : policy;
}

/** Display paths hide machine-specific absolute directories without changing download identity. */
export function previewPath(file: PreviewFile) {
  return file.reference.startsWith("/") ? file.name : file.reference.replace(/^\.\//, "");
}
export function displayPreviewFiles(files: PreviewFile[], query = "") {
  const search = query.trim().toLocaleLowerCase();
  return files.filter(
    (file) =>
      !file.resource &&
      (!search || `${file.name} ${previewPath(file)}`.toLocaleLowerCase().includes(search)),
  );
}
export function selectedPreviewFile(files: PreviewFile[], selected: string | null) {
  const visible = displayPreviewFiles(files);
  return (
    visible.find((file) => file.key === selected) ??
    visible.find((file) => file.available) ??
    visible[0]
  );
}
