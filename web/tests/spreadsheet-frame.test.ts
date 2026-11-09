import assert from "node:assert/strict";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import { spreadsheetFrameDocument } from "../lib/kyler/spreadsheet-frame.ts";

const scripts = ["/* Vue */", "/* VueDemi */", "/* Excel */"];
const css = "body{margin:0}";
test("spreadsheet frame isolates vendor code, permits local blobs and blocks remote connections", () => {
  const document = spreadsheetFrameDocument("test-channel", scripts, css);
  assert.ok(document.includes("connect-src blob:"));
  assert.ok(document.includes("script-src 'nonce-test-channel'"));
  assert.ok(!document.includes("unsafe-eval"));
  assert.throws(() => spreadsheetFrameDocument('bad"channel', scripts, css));
  assert.throws(() => spreadsheetFrameDocument("test", [], css));
  const escaped = spreadsheetFrameDocument(
    "test",
    ["console.log('</script><img src=x>')", ...scripts.slice(1)],
    "/* </style><img src=x> */",
  );
  assert.ok(!escaped.includes("</script><img"));
  assert.ok(!escaped.includes("</style><img"));
});

test("frame accepts one workbook from its parent and reports lifecycle without exposing errors", () => {
  const html = spreadsheetFrameDocument("test-channel", scripts, css);
  const code = [...html.matchAll(/<script nonce="test-channel">([\s\S]*?)<\/script>/g)].at(-1)![1];
  const messages: { status: string }[] = [];
  let receive: ((event: unknown) => void) | undefined;
  let mounted = 0;
  let props: Record<string, unknown> = {};
  const app = {
    config: { errorHandler: () => undefined },
    mount: () => {
      mounted++;
    },
  };
  const parent = { postMessage: (value: { status: string }) => messages.push(value) };
  runInNewContext(code, {
    ArrayBuffer,
    parent,
    document: { body: { dataset: { channel: "test-channel" } }, getElementById: () => ({}) },
    window: {
      addEventListener: (_: string, listener: typeof receive) => {
        receive = listener;
      },
      Vue: {
        createApp: (options: { render: () => unknown }) => {
          options.render();
          return app;
        },
        h: (_: unknown, value: Record<string, unknown>) => {
          props = value;
        },
      },
      "vue-office-excel": {},
    },
  });
  assert.equal(messages[0].status, "ready");
  const buffer = new ArrayBuffer(8);
  const data = { type: "kyler-spreadsheet-open", channel: "test-channel", buffer };
  receive!({ source: {}, data });
  receive!({ source: parent, data: { ...data, channel: "other" } });
  assert.equal(mounted, 0);
  receive!({ source: parent, data });
  receive!({ source: parent, data });
  assert.equal(mounted, 1);
  assert.equal(props.src, buffer);
  assert.equal((props.options as { showContextmenu: boolean }).showContextmenu, false);
  (props.onRendered as () => void)();
  (props.onError as (error: Error) => void)(new Error("private workbook contents"));
  assert.deepEqual(
    messages.map((message) => message.status),
    ["ready", "rendered", "error"],
  );
  assert.ok(!JSON.stringify(messages).includes("private workbook contents"));
});
