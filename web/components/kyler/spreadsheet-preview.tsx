"use client";
import { useEffect, useState } from "react";
import type { SpreadsheetSheet } from "@/lib/kyler/spreadsheet";

export function SpreadsheetPreview({ bytes }: { bytes: Uint8Array }) {
  const [sheets, setSheets] = useState<SpreadsheetSheet[]>([]);
  const [selected, setSelected] = useState(0);
  const [error, setError] = useState("");
  useEffect(() => {
    const worker = new Worker(new URL("../../lib/kyler/spreadsheet-worker.ts", import.meta.url), {
      type: "module",
    });
    const timer = setTimeout(() => {
      worker.terminate();
      setError("Workbook preview timed out. Download the original file.");
    }, 15000);
    worker.onmessage = (event) => {
      clearTimeout(timer);
      if (event.data.error) setError(event.data.error);
      else {
        setSheets(event.data.sheets);
        setSelected(0);
      }
      worker.terminate();
    };
    worker.onerror = () => {
      clearTimeout(timer);
      worker.terminate();
      setError("Workbook preview could not load.");
    };
    const buffer = bytes.slice().buffer;
    worker.postMessage(buffer, [buffer]);
    return () => {
      clearTimeout(timer);
      worker.terminate();
    };
  }, [bytes]);
  if (error)
    return (
      <p role="alert" className="p-5 text-sm text-muted-foreground">
        {error}
      </p>
    );
  if (!sheets.length)
    return (
      <p role="status" className="p-5 text-sm text-muted-foreground">
        Reading workbook…
      </p>
    );
  const sheet = sheets[selected];
  const columns = Math.max(0, ...sheet.rows.map((row) => row.length));
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-collapse text-left text-xs">
          <thead className="sticky top-0 bg-muted">
            <tr>
              <th className="border-b border-r border-border px-2 py-2 text-muted-foreground" />
              {Array.from({ length: columns }, (_, index) => (
                <th key={index} className="border-b border-r border-border px-3 py-2 font-medium">
                  {columnName(index)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {sheet.rows.map((row, index) => (
              <tr key={index}>
                <th className="border-b border-r border-border bg-muted/30 px-2 py-2 font-normal text-muted-foreground">
                  {index + 1}
                </th>
                {Array.from({ length: columns }, (_, column) => (
                  <td
                    className="max-w-80 whitespace-pre-wrap break-words border-b border-r border-border/50 px-3 py-2"
                    key={column}
                  >
                    {row[column] ?? ""}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-1 border-t border-border/60 p-2">
        {sheets.map((item, index) => (
          <button
            key={index}
            className={`rounded px-3 py-1.5 text-xs ${selected === index ? "bg-muted font-medium" : "text-muted-foreground hover:bg-muted/50"}`}
            onClick={() => setSelected(index)}
          >
            {item.name}
          </button>
        ))}
        {sheet.truncated && (
          <span className="px-2 text-[10px] text-muted-foreground">
            Preview limited to 200 rows and 100 columns.
          </span>
        )}
      </div>
    </div>
  );
}
function columnName(index: number) {
  let name = "";
  for (let n = index + 1; n > 0; n = Math.floor((n - 1) / 26))
    name = String.fromCharCode(65 + ((n - 1) % 26)) + name;
  return name;
}
