import assert from "node:assert/strict";
import test from "node:test";
import { unzipSync, strFromU8 } from "fflate";
import { attachments } from "../lib/kyler/client.ts";
import {
  attachPastedText,
  LONG_PASTE_CHARACTERS,
  pastedTextDocument,
  shouldAttachPastedText,
  WORD_DOCUMENT_TYPE,
} from "../lib/kyler/pasted-document.ts";

test("only long plain text is converted; Unicode characters count once", () => {
  assert.equal(shouldAttachPastedText("x".repeat(LONG_PASTE_CHARACTERS)), false);
  assert.equal(shouldAttachPastedText("中".repeat(LONG_PASTE_CHARACTERS + 1)), true);
  assert.equal(shouldAttachPastedText("😀".repeat(LONG_PASTE_CHARACTERS)), false);
  assert.equal(shouldAttachPastedText("😀".repeat(LONG_PASTE_CHARACTERS + 1)), true);
  assert.equal(shouldAttachPastedText("x".repeat(5000), true), false);
});

test("Word package preserves paragraphs, tabs, spaces and Unicode while escaping XML", async () => {
  const file = pastedTextDocument('  中😀 <script>& "quoted"\r\n\r\nlast\tcolumn  ');
  assert.equal(file.type, WORD_DOCUMENT_TYPE);
  assert.match(file.name, /^pasted-text-.+\.docx$/);
  const parts = unzipSync(new Uint8Array(await file.arrayBuffer()));
  assert.deepEqual(Object.keys(parts).sort(), [
    "[Content_Types].xml",
    "_rels/.rels",
    "word/document.xml",
  ]);
  assert.match(strFromU8(parts["[Content_Types].xml"]), /wordprocessingml.document.main\+xml/);
  assert.match(strFromU8(parts["_rels/.rels"]), /Target="word\/document.xml"/);
  const document = strFromU8(parts["word/document.xml"]);
  assert.equal((document.match(/<w:p>/g) ?? []).length, 3);
  assert.match(document, /xml:space="preserve">  中😀 &lt;script&gt;&amp; "quoted"/);
  assert.match(document, /<w:tab\/>/);
  assert.match(document, /column  <\/w:t>/);
  assert.ok(!document.includes("<script>"));
  const cleaned = strFromU8(
    unzipSync(new Uint8Array(await pastedTextDocument("a\u0000b\ud800c").arrayBuffer()))[
      "word/document.xml"
    ],
  );
  assert.ok(cleaned.includes("a�b�c"));
});

test("oversized Word XML is rejected before uploading", async () => {
  let uploaded = false;
  let text = "existing";
  const pasted = "&".repeat(4 * 1024 * 1024);
  const attached = await attachPastedText(
    {
      getState: () => ({ text }),
      setText: (value) => {
        text = value;
      },
      addAttachment: async () => {
        uploaded = true;
      },
    },
    pasted,
    8,
    8,
  );
  assert.equal(attached, false);
  assert.equal(uploaded, false);
  assert.equal(text, "existing" + pasted);
});

test("successful conversion uploads through the attachment adapter without changing the draft", async (t) => {
  let requests = 0;
  const adapter = attachments("session");
  const text = "Keep my instructions";
  t.mock.method(globalThis, "fetch", async (url: string, init?: RequestInit) => {
    requests++;
    assert.match(
      url,
      /^\/api\/kyler\/sessions\/session\/attachments\?filename=pasted-text-.+\.docx$/,
    );
    assert.equal(init?.method, "POST");
    assert.ok(init?.body instanceof File);
    assert.equal(init.body.type, WORD_DOCUMENT_TYPE);
    const xml = strFromU8(
      unzipSync(new Uint8Array(await init.body.arrayBuffer()))["word/document.xml"],
    );
    assert.ok(xml.includes("long pasted content"));
    return Response.json({ id: "document" });
  });
  let attachedName = "";
  const result = await attachPastedText(
    {
      getState: () => ({ text }),
      setText: () => {
        assert.fail("Successful paste must retain the draft");
      },
      addAttachment: async (file) => {
        const pending = await adapter.add({ file } as never);
        assert.ok(!(Symbol.asyncIterator in pending));
        attachedName = file.name;
      },
    },
    "long pasted content",
    0,
    4,
  );
  assert.equal(result, true);
  assert.match(attachedName, /\.docx$/);
  assert.equal(requests, 1);
});

test("upload failures restore the paste at the cursor without losing newer edits", async () => {
  let text = "before selection after";
  const composer = {
    getState: () => ({ text }),
    setText: (value: string) => {
      text = value;
    },
    addAttachment: async () => {
      throw new Error("Offline");
    },
  };
  assert.equal(await attachPastedText(composer, "restored", 7, 16), false);
  assert.equal(text, "before restored after");
  composer.addAttachment = async () => {
    text = "New instructions";
    throw new Error("Offline");
  };
  assert.equal(await attachPastedText(composer, "pasted content", 0, 0), false);
  assert.equal(text, "New instructions\npasted content");
});
