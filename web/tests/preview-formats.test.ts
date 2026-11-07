import assert from "node:assert/strict";
import { test } from "node:test";
import {
  autoPreview,
  initialBrowserState,
  type PreviewFile,
} from "../lib/kyler/artifact-preview.ts";
test("previewable generated text opens the panel at a 70/30 split", () => {
  const file: PreviewFile = {
    key: "source",
    id: "source",
    session: "s",
    name: "main.rs",
    href: "/content",
    reference: "main.rs",
    group: "generated",
    execution: "run",
    bytes: 10,
    available: true,
    kind: "text",
  };
  const opened = autoPreview([file], new Set(), "s", { ...initialBrowserState, width: 45 });
  assert.equal(opened.open, true);
  assert.equal(opened.width, 70);
});

test("XLSX is a previewable spreadsheet format", async () => {
  const { previewKind } = await import("../lib/kyler/artifact-preview.ts");
  assert.equal(previewKind("report.xlsx"), "spreadsheet");
});

test("workbook previews reject invalid archives and expansion bombs", async () => {
  const { validateWorkbook } = await import("../lib/kyler/spreadsheet.ts");
  assert.throws(() => validateWorkbook(new Uint8Array(20)));
  const archive = new Uint8Array(68);
  const view = new DataView(archive.buffer);
  view.setUint32(0, 0x02014b50, true);
  view.setUint32(46, 0x06054b50, true);
  view.setUint16(56, 1, true);
  view.setUint32(62, 0, true);
  view.setUint32(24, 1024, true);
  assert.doesNotThrow(() => validateWorkbook(archive));
  view.setUint32(24, 32 * 1024 * 1024, true);
  assert.throws(() => validateWorkbook(archive));
});
