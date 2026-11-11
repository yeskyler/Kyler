import { randomUuid } from "./random-uuid.ts";
import { strToU8, zipSync } from "fflate";

export const LONG_PASTE_CHARACTERS = 4000;
export const WORD_DOCUMENT_TYPE =
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const MAX_DOCUMENT_XML_BYTES = 16 * 1024 * 1024;

export function shouldAttachPastedText(text: string, hasFiles = false) {
  if (hasFiles) return false;
  let characters = 0;
  for (const _ of text) if (++characters > LONG_PASTE_CHARACTERS) return true;
  return false;
}

function xmlText(text: string) {
  return text
    .replace(/[^\t\n\r\u0020-\ud7ff\ue000-\ufffd\u{10000}-\u{10ffff}]/gu, "\ufffd")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

export function pastedTextDocument(text: string, now = new Date()) {
  if (strToU8(text).length > MAX_DOCUMENT_XML_BYTES)
    throw new Error("The pasted text exceeds the document size limit.");
  const paragraphs = text
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((line) => {
      const runs = line
        .split("\t")
        .map((part) => `<w:t xml:space="preserve">${xmlText(part)}</w:t>`)
        .join("<w:tab/>");
      return `<w:p><w:r>${runs}</w:r></w:p>`;
    })
    .join("");
  const document = strToU8(
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>${paragraphs}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>`,
  );
  if (document.length > MAX_DOCUMENT_XML_BYTES)
    throw new Error("The pasted text exceeds the document size limit.");
  const bytes = zipSync(
    {
      "[Content_Types].xml": strToU8(
        '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>',
      ),
      "_rels/.rels": strToU8(
        '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>',
      ),
      "word/document.xml": document,
    },
    { level: 0 },
  );
  return new File(
    [bytes],
    `pasted-text-${now.toISOString().replace(/[:.]/g, "-")}-${randomUuid().slice(0, 8)}.docx`,
    { type: WORD_DOCUMENT_TYPE },
  );
}

export function restorePastedText(
  current: string,
  original: string,
  text: string,
  start: number,
  end: number,
) {
  return current === original
    ? current.slice(0, start) + text + current.slice(end)
    : current + (current && !current.endsWith("\n") ? "\n" : "") + text;
}

export async function attachPastedText(
  composer: {
    getState: () => { text: string };
    setText: (text: string) => void;
    addAttachment: (file: File) => Promise<void>;
  },
  text: string,
  start: number,
  end: number,
) {
  const original = composer.getState().text;
  try {
    await composer.addAttachment(pastedTextDocument(text));
    return true;
  } catch {
    composer.setText(restorePastedText(composer.getState().text, original, text, start, end));
    return false;
  }
}
