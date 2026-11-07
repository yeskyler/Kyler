import readWorkbook from "read-excel-file/universal";
import { validateWorkbook, type SpreadsheetSheet } from "./spreadsheet";

self.onmessage = async (event: MessageEvent<ArrayBuffer>) => {
  try {
    const bytes = new Uint8Array(event.data);
    validateWorkbook(bytes);
    const workbook = await readWorkbook(new Blob([bytes]));
    const sheets: SpreadsheetSheet[] = workbook.slice(0, 20).map((sheet) => ({
      name: sheet.sheet,
      rows: sheet.data
        .slice(0, 200)
        .map((row) =>
          row
            .slice(0, 100)
            .map((cell) =>
              cell instanceof Date ? cell.toISOString() : cell === null ? "" : String(cell),
            ),
        ),
      truncated: sheet.data.length > 200 || sheet.data.some((row) => row.length > 100),
    }));
    self.postMessage({ sheets });
  } catch {
    self.postMessage({
      error:
        "This workbook could not be previewed. Download it to open in a spreadsheet application.",
    });
  }
};
