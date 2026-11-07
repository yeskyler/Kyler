/** Bound ZIP expansion before an XLSX reader allocates workbook contents. */
export function validateWorkbook(bytes: Uint8Array) {
  if (bytes.length > 5 * 1024 * 1024)
    throw new Error("Spreadsheet preview is limited to 5 MiB. Download the original workbook.");
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let footer = -1;
  for (let offset = bytes.length - 22; offset >= Math.max(0, bytes.length - 65557); offset--) {
    if (view.getUint32(offset, true) === 0x06054b50) {
      footer = offset;
      break;
    }
  }
  if (footer < 0) throw new Error("This file is not a supported XLSX workbook.");
  const count = view.getUint16(footer + 10, true);
  let offset = view.getUint32(footer + 16, true);
  let total = 0;
  if (count > 512) throw new Error("This workbook contains too many entries to preview.");
  for (let index = 0; index < count; index++) {
    if (offset + 46 > footer || view.getUint32(offset, true) !== 0x02014b50)
      throw new Error("Invalid workbook archive.");
    const size = view.getUint32(offset + 24, true);
    total += size;
    if (size > 16 * 1024 * 1024 || total > 32 * 1024 * 1024)
      throw new Error("This workbook is too large to expand for preview.");
    offset +=
      46 +
      view.getUint16(offset + 28, true) +
      view.getUint16(offset + 30, true) +
      view.getUint16(offset + 32, true);
  }
}
export type SpreadsheetSheet = { name: string; rows: string[][]; truncated: boolean };
