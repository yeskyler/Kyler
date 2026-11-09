import assert from "node:assert/strict";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import { zipSync, strToU8 } from "fflate";
import { presentationFrameDocument } from "../lib/kyler/presentation-frame.ts";
import { validatePresentation } from "../lib/kyler/spreadsheet.ts";

const scripts = ["/* Vue */", "/* Demi */", "/* PPTX */"];
test("presentation archive validation rejects unrelated files, excessive slides and expansion bombs", () => {
  const archive = (slides: number) =>
    zipSync(
      Object.fromEntries([
        ["ppt/presentation.xml", strToU8("<presentation/>")],
        ...Array.from({ length: slides }, (_, index) => [
          `ppt/slides/slide${index + 1}.xml`,
          strToU8("<slide/>"),
        ]),
      ]),
    );
  assert.doesNotThrow(() => validatePresentation(archive(2)));
  assert.throws(() => validatePresentation(new Uint8Array([1, 2])), /PPTX/);
  assert.throws(
    () => validatePresentation(zipSync({ "xl/workbook.xml": strToU8("workbook") })),
    /PPTX/,
  );
  assert.throws(() => validatePresentation(archive(101)), /100 slides/);
  const bomb = archive(1);
  const view = new DataView(bomb.buffer, bomb.byteOffset, bomb.byteLength);
  for (let index = 0; index < bomb.length - 46; index++) {
    if (view.getUint32(index, true) === 0x02014b50) {
      view.setUint32(index + 24, 17 * 1024 * 1024, true);
      break;
    }
  }
  assert.throws(() => validatePresentation(bomb), /expand/);
});

test("presentation frame restricts executable code to trusted resources", () => {
  const html = presentationFrameDocument("channel", [
    "console.log('</script><img src=x>')",
    ...scripts.slice(1),
  ]);
  assert.ok(!html.includes("</script><img"));
  assert.ok(html.includes("script-src 'nonce-channel'"));
  assert.ok(html.includes("connect-src blob:"));
  assert.ok(!html.includes("unsafe-eval"));
  assert.throws(() => presentationFrameDocument('bad"channel', scripts));
});

test("presentation controls validate sender and preserve slide ratio without reparsing", () => {
  const html = presentationFrameDocument("channel", scripts);
  const code = [...html.matchAll(/<script nonce="channel">([\s\S]*?)<\/script>/g)].at(-1)![1];
  const slides = [0, 1].map(() => ({
    style: {} as Record<string, string>,
    offsetWidth: 960,
    offsetHeight: 720,
  }));
  const wrapper = { style: {} };
  const deck = {
    style: {} as Record<string, string>,
    querySelectorAll: () => slides,
    querySelector: () => wrapper,
  };
  const surface = { style: {} };
  const viewport = { clientWidth: 800, clientHeight: 600, scrollTop: 0, scrollLeft: 0 };
  const elements = { deck, surface, viewport };
  const messages: { status: string; page: number; zoom: number }[] = [];
  const listeners: Record<string, (event?: unknown) => void> = {};
  let props: Record<string, unknown> = {};
  let mounts = 0;
  const parent = { postMessage: (message: (typeof messages)[number]) => messages.push(message) };
  runInNewContext(code, {
    ArrayBuffer,
    parent,
    document: {
      body: { dataset: { channel: "channel" } },
      getElementById: (name: keyof typeof elements) => elements[name],
    },
    window: {
      addEventListener: (name: string, listener: (typeof listeners)[string]) => {
        listeners[name] = listener;
      },
      Vue: {
        createApp: (options: { render: () => unknown }) => {
          options.render();
          return {
            mount: () => {
              mounts++;
            },
            config: {},
          };
        },
        h: (_: unknown, value: Record<string, unknown>) => {
          props = value;
        },
      },
      "vue-office-pptx": {},
    },
  });
  const data = { type: "kyler-presentation-open", channel: "channel", buffer: new ArrayBuffer(4) };
  listeners.message({ source: {}, data });
  assert.equal(mounts, 0);
  listeners.message({ source: parent, data });
  listeners.message({ source: parent, data });
  assert.equal(mounts, 1);
  (props.onRendered as () => void)();
  assert.equal(deck.style.height, "720px");
  listeners.message({
    source: parent,
    data: { type: "kyler-presentation-control", channel: "wrong", page: 2 },
  });
  assert.equal(messages.at(-1)?.page, 1);
  listeners.message({
    source: parent,
    data: { type: "kyler-presentation-control", channel: "channel", page: 2, zoom: 2 },
  });
  assert.equal(messages.at(-1)?.page, 2);
  assert.equal(deck.style.transform, "scale(2)");
  assert.equal(slides[0].style.display, "none");
  listeners.message({
    source: parent,
    data: { type: "kyler-presentation-control", channel: "channel", page: 99, fit: true },
  });
  assert.equal(messages.at(-1)?.page, 2);
  viewport.clientWidth = 400;
  listeners.resize();
  assert.ok(messages.at(-1)!.zoom < 0.5);
  assert.equal(mounts, 1);
});

test("preview preparation removes only dangling declarations and preserves source bytes", async () => {
  const { preparePresentation } = await import("../lib/kyler/presentation.ts");
  const { unzipSync, strFromU8 } = await import("fflate");
  const metadata =
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/ppt/slides/slide1.xml" ContentType="slide"/><Override PartName="/ppt/slideMasters/missing.xml" ContentType="master"/></Types>';
  const source = zipSync({
    "[Content_Types].xml": strToU8(metadata),
    "ppt/presentation.xml": strToU8("<presentation/>"),
    "ppt/slides/slide1.xml": strToU8("<slide>original</slide>"),
  });
  const original = source.slice();
  const prepared = preparePresentation(source);
  assert.deepEqual(source, original);
  const files = unzipSync(prepared);
  assert.ok(!strFromU8(files["[Content_Types].xml"]).includes("missing.xml"));
  assert.ok(strFromU8(files["[Content_Types].xml"]).includes("slide1.xml"));
  assert.equal(strFromU8(files["ppt/slides/slide1.xml"]), "<slide>original</slide>");
  assert.equal(preparePresentation(prepared), prepared);
});
