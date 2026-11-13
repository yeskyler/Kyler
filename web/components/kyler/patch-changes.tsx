"use client";

import type { PatchResult } from "@/lib/kyler/patch-changes";

const labels = {
  added: ["Added", "text-emerald-700 dark:text-emerald-400"],
  modified: ["Modified", "text-blue-700 dark:text-blue-400"],
  deleted: ["Deleted", "text-red-700 dark:text-red-400"],
  renamed: ["Renamed", "text-muted-foreground"],
} as const;

export function PatchCounts({ added, removed }: { added: number; removed: number }) {
  return (
    <span className="inline-flex shrink-0 gap-2 font-mono tabular-nums">
      {added > 0 && (
        <span
          className="text-emerald-700 dark:text-emerald-400"
          aria-label={`${added} lines added`}
        >
          +{added}
        </span>
      )}
      {removed > 0 && (
        <span className="text-red-700 dark:text-red-400" aria-label={`${removed} lines removed`}>
          −{removed}
        </span>
      )}
    </span>
  );
}

export function PatchSummary({ result }: { result: PatchResult }) {
  return (
    <span className="ml-auto inline-flex min-w-0 items-center gap-2 text-xs">
      {result.status === "partial" && (
        <span className="shrink-0 text-amber-700 dark:text-amber-400">Partial</span>
      )}
      <span className="shrink-0 text-muted-foreground">
        {result.files.length} {result.files.length === 1 ? "file" : "files"}
      </span>
      <PatchCounts
        added={result.files.reduce((sum, file) => sum + file.added, 0)}
        removed={result.files.reduce((sum, file) => sum + file.removed, 0)}
      />
    </span>
  );
}

function FilePath({ path }: { path: string }) {
  const slash = path.lastIndexOf("/");
  return (
    <span className="inline-flex min-w-0 max-w-full font-mono" title={path}>
      {slash >= 0 && (
        <span className="min-w-0 truncate text-muted-foreground">{path.slice(0, slash + 1)}</span>
      )}
      <span className="shrink-0 max-w-full truncate">{path.slice(slash + 1)}</span>
    </span>
  );
}

export function PatchFiles({ result }: { result: PatchResult }) {
  return (
    <div className="space-y-2 text-xs">
      {result.status === "partial" && (
        <p className="text-amber-700 dark:text-amber-400">
          Partially applied. Only confirmed changes are listed.
        </p>
      )}
      {result.error && <p className="break-words text-destructive">{result.error}</p>}
      <ul aria-label="Changed files" className="space-y-1.5">
        {result.files.map((file, index) => {
          const [label, color] = labels[file.operation];
          return (
            <li key={`${file.path}-${index}`} className="flex min-w-0 items-center gap-2">
              <span className={`w-14 shrink-0 text-[10px] font-medium ${color}`}>{label}</span>
              <span
                className="min-w-0 flex-1"
                title={file.previous_path ? `${file.previous_path} → ${file.path}` : file.path}
              >
                {file.previous_path && (
                  <span className="flex min-w-0 items-center gap-1 text-muted-foreground">
                    <FilePath path={file.previous_path} />
                    <span>→</span>
                  </span>
                )}
                <FilePath path={file.path} />
              </span>
              <PatchCounts added={file.added} removed={file.removed} />
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export function ComposerPatchSummary({ result }: { result: PatchResult }) {
  return (
    <span className="inline-flex max-w-full min-w-0 items-center justify-center gap-2 text-xs">
      {result.status === "partial" && (
        <span className="shrink-0 text-amber-700 dark:text-amber-400">Partial</span>
      )}
      {result.files.length === 1 ? (
        <FilePath path={result.files[0].path} />
      ) : (
        <span className="shrink-0">{result.files.length} files</span>
      )}
      <PatchCounts
        added={result.files.reduce((sum, file) => sum + file.added, 0)}
        removed={result.files.reduce((sum, file) => sum + file.removed, 0)}
      />
    </span>
  );
}
