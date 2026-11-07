"use client";
import { useEffect, useState } from "react";
import { ChevronDownIcon, ChevronRightIcon, FileIcon, FolderIcon, SearchIcon } from "lucide-react";
import {
  displayPreviewFiles,
  previewPath,
  selectedPreviewFile,
  type PreviewFile,
} from "@/lib/kyler/artifact-preview";
import { useArtifactBrowser } from "./artifact-browser";

const row =
  "group flex w-full min-w-0 items-center gap-1.5 rounded px-1.5 py-1.5 text-left text-[11px] hover:bg-muted outline-none focus-visible:ring-2 focus-visible:ring-ring";
export function WorkspaceTree() {
  const browser = useArtifactBrowser();
  const [query, setQuery] = useState("");
  const files = displayPreviewFiles(browser.files, query).map((file) => ({
    ...file,
    reference: previewPath(file),
  }));
  return (
    <nav
      aria-label="Session directory tree"
      className="w-36 shrink-0 overflow-auto border-r border-border/60 bg-muted/10 p-2 sm:w-40"
    >
      <label className="mb-3 flex items-center gap-1.5 rounded-md border border-border/60 bg-background px-2 py-1.5 text-muted-foreground focus-within:ring-1 focus-within:ring-ring">
        <SearchIcon className="size-3 shrink-0" />
        <input
          aria-label="Find files"
          placeholder="Find files"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape" && query) {
              event.preventDefault();
              event.stopPropagation();
              setQuery("");
            }
          }}
          className="w-full min-w-0 bg-transparent text-xs outline-none"
        />
      </label>
      <FileGroup
        searching={!!query.trim()}
        label="Inputs"
        files={files.filter((file) => file.group === "uploaded")}
      />
      <FileGroup
        searching={!!query.trim()}
        label="Outputs"
        files={files.filter((file) => file.group === "generated")}
      />
      {query && !files.length && (
        <p className="px-2 py-3 text-xs text-muted-foreground">No matching files.</p>
      )}
    </nav>
  );
}
function FileGroup({
  label,
  files,
  searching,
}: {
  label: string;
  files: PreviewFile[];
  searching: boolean;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div className="mb-2">
      <button
        className={`${row} font-medium`}
        onClick={() => setOpen(!open)}
        aria-expanded={open || searching}
      >
        {open || searching ? (
          <ChevronDownIcon className="size-3 shrink-0" />
        ) : (
          <ChevronRightIcon className="size-3 shrink-0" />
        )}
        {label}
        <span className="ml-auto text-muted-foreground">{files.length || ""}</span>
      </button>
      {(open || searching) && (
        <VirtualDirectory files={files} prefix="" depth={1} searching={searching} />
      )}
    </div>
  );
}
function VirtualDirectory({
  files,
  prefix,
  depth,
  searching,
}: {
  files: PreviewFile[];
  prefix: string;
  depth: number;
  searching: boolean;
}) {
  const browser = useArtifactBrowser();
  const selected = selectedPreviewFile(browser.files, browser.state.selected);
  const selectedPath = selected ? previewPath(selected) : "";
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  useEffect(() => {
    if (!selectedPath.startsWith(prefix)) return;
    const rest = selectedPath.slice(prefix.length);
    if (rest.includes("/")) setExpanded((previous) => new Set([...previous, rest.split("/")[0]]));
  }, [selectedPath, prefix]);
  const folders = [
    ...new Set(
      files
        .map((file) =>
          file.reference.startsWith(prefix)
            ? file.reference.slice(prefix.length).split("/").slice(0, -1)[0]
            : undefined,
        )
        .filter((value): value is string => !!value),
    ),
  ];
  const leaves = files.filter(
    (file) =>
      file.reference.startsWith(prefix) && !file.reference.slice(prefix.length).includes("/"),
  );
  return (
    <>
      {folders.map((folder) => (
        <div key={folder}>
          <button
            style={{ paddingLeft: depth * 10 }}
            className={row}
            aria-expanded={expanded.has(folder) || searching}
            onClick={() =>
              setExpanded((previous) => {
                const next = new Set(previous);
                if (next.has(folder)) next.delete(folder);
                else next.add(folder);
                return next;
              })
            }
          >
            <FolderIcon className="size-3 shrink-0" />
            <span className="truncate">{folder}</span>
          </button>
          {(expanded.has(folder) || searching) && (
            <VirtualDirectory
              files={files}
              prefix={`${prefix}${folder}/`}
              depth={depth + 1}
              searching={searching}
            />
          )}
        </div>
      ))}
      {leaves.map((file) => (
        <button
          key={file.key}
          style={{ paddingLeft: depth * 10 }}
          className={`${row} ${file.key === selected?.key ? "bg-muted font-medium" : "text-muted-foreground"}`}
          aria-current={file.key === selected?.key ? "true" : undefined}
          title={file.reference}
          onClick={() => browser.openFile(file.key)}
        >
          <FileIcon className="size-3 shrink-0" />
          <span className="truncate">{file.name}</span>
        </button>
      ))}
    </>
  );
}
