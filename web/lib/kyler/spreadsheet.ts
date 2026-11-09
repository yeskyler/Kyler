/** Inspect ZIP metadata before an Office parser allocates expanded contents. */
function validateArchive(bytes: Uint8Array, presentation: boolean) {
  const label = presentation ? "Presentation" : "Spreadsheet";
  const limit = presentation ? 20 : 5;
  if (bytes.length > limit * 1024 * 1024)
    throw new Error(`${label} preview is limited to ${limit} MiB. Download the original file.`);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let footer = -1;
  for (let offset = bytes.length - 22; offset >= Math.max(0, bytes.length - 65557); offset--) {
    if (view.getUint32(offset, true) === 0x06054b50) {
      footer = offset;
      break;
    }
  }
  if (footer < 0)
    throw new Error(
      presentation
        ? "This file is not a supported PPTX presentation."
        : "This file is not a supported XLSX workbook.",
    );
  const count = view.getUint16(footer + 10, true);
  let offset = view.getUint32(footer + 16, true);
  let total = 0;
  const names: string[] = [];
  if (count > (presentation ? 2048 : 512))
    throw new Error(`${label} contains too many entries to preview.`);
  for (let index = 0; index < count; index++) {
    if (offset + 46 > footer || view.getUint32(offset, true) !== 0x02014b50)
      throw new Error("Invalid Office archive.");
    const size = view.getUint32(offset + 24, true);
    total += size;
    if (size > 16 * 1024 * 1024 || total > (presentation ? 64 : 32) * 1024 * 1024)
      throw new Error(`${label} is too large to expand for preview.`);
    const nameLength = view.getUint16(offset + 28, true);
    const next =
      offset +
      46 +
      nameLength +
      view.getUint16(offset + 30, true) +
      view.getUint16(offset + 32, true);
    if (next > footer) throw new Error("Invalid Office archive.");
    if (presentation && view.getUint16(offset + 8, true) & 1)
      throw new Error("Encrypted presentations cannot be previewed. Download the original file.");
    names.push(new TextDecoder().decode(bytes.subarray(offset + 46, offset + 46 + nameLength)));
    offset = next;
  }
  return names;
}

export function validateWorkbook(bytes: Uint8Array) {
  validateArchive(bytes, false);
}

export function validatePresentation(bytes: Uint8Array) {
  const names = validateArchive(bytes, true);
  const count = names.filter((name) => /^ppt\/slides\/slide\d+\.xml$/.test(name)).length;
  if (!names.includes("ppt/presentation.xml") || !count)
    throw new Error("This file is not a supported PPTX presentation.");
  if (count > 100)
    throw new Error("Presentation preview is limited to 100 slides. Download the original file.");
  return names;
}
