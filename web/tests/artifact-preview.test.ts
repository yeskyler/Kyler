import assert from "node:assert/strict";
import { test } from "node:test";
import {
  autoPreview,
  displayPreviewFiles,
  selectedPreviewFile,
  previewPath,
  htmlPolicy,
  initialBrowserState,
  parseCsv,
  previewKind,
  previewResource,
  restoreBrowserState,
  type PreviewFile,
} from "../lib/kyler/artifact-preview.ts";
import { prepareHtml, readPreview, MAX_TEXT } from "../lib/kyler/preview-content.ts";
const file: PreviewFile = {
  key: "generated:s:a",
  id: "a",
  session: "s",
  name: "index.html",
  href: "/content/a",
  reference: "report/index.html",
  group: "generated",
  execution: "run",
  bytes: 100,
  available: true,
  kind: "html",
  primary: true,
};

test("preview types recognize documents, images and unsupported archives", () => {
  assert.equal(previewKind("report.PDF"), "pdf");
  assert.equal(previewKind("icon.svg"), "svg");
  assert.equal(previewKind("deck.pptx"), "slides");
});
test("new outputs open once without stealing focus or reopening dismissed work", () => {
  assert.equal(autoPreview([file], new Set(), "s", initialBrowserState).selected, file.key);
  assert.equal(autoPreview([file], new Set(), "other", initialBrowserState), initialBrowserState);
  assert.equal(
    autoPreview([file], new Set([file.key]), "s", initialBrowserState),
    initialBrowserState,
  );
  const open = { ...initialBrowserState, open: true, selected: "other" };
  assert.equal(autoPreview([file], new Set(), "s", open), open);
  const dismissed = { ...initialBrowserState, dismissed: true };
  assert.equal(autoPreview([file], new Set(), "s", dismissed), dismissed);
  assert.equal(restoreBrowserState({ width: 999, open: true }).width, 70);
  assert.equal(restoreBrowserState({ width: NaN }).width, 70);
});
test("CSV keeps quoted newlines and escaped quotes and bounds rows", () => {
  assert.deepEqual(parseCsv('name,value\r\n"a,b","line\n""two"""\r\n').rows, [
    ["name", "value"],
    ["a,b", 'line\n"two"'],
  ]);
  assert.equal(parseCsv("a\nb\nc", 2).truncated, true);
});
test("HTML resources require retained snapshots from the same owner and execution", () => {
  const sibling = {
    ...file,
    key: "css",
    reference: "report/style.css",
    name: "style.css",
    kind: "text" as const,
  };
  assert.equal(previewResource([sibling], file, "style.css?v=1"), sibling);
  for (const reference of [
    "https://example.com/a",
    "//example.com/a",
    "/etc/passwd",
    "../../secret",
    "javascript:alert(1)",
  ])
    assert.equal(previewResource([sibling], file, reference), undefined);
  assert.equal(previewResource([{ ...sibling, session: "other" }], file, "style.css"), undefined);
  assert.equal(previewResource([{ ...sibling, execution: "other" }], file, "style.css"), undefined);
});
test("HTML preprocessing strips navigation and external resources before embedding", async () => {
  const output = await prepareHtml(
    '<base href="https://example.com"><meta http-equiv="refresh" content="0;url=https://example.com"><iframe src="https://example.com"></iframe><link rel="stylesheet" href="https://example.com/style.css"><img src="https://example.com/a.png"><a href="https://example.com">link</a><script src="https://example.com/a.js"></script><h1>Hello</h1>',
    file,
    [file],
    false,
    new AbortController().signal,
    () => {
      throw new Error("Unexpected resource");
    },
  );
  assert.ok(output.includes("<h1>Hello</h1>"));
  assert.ok(!output.includes("https://example.com"));
  assert.ok(!output.includes("<iframe"));
  assert.ok(output.includes("script-src 'none'"));
  assert.ok(htmlPolicy(true).includes("script-src 'unsafe-inline' blob:"));
});
test("text preview is bounded without changing the downloadable source", async (context) => {
  context.mock.method(globalThis, "fetch", async () => new Response("x".repeat(MAX_TEXT + 20)));
  const result = await readPreview({ ...file, kind: "text" }, new AbortController().signal);
  assert.equal(result.truncated, true);
  assert.equal(result.bytes.length, MAX_TEXT);
});

test("retained HTML siblings are fetched once and rewritten to owned object URLs", async (context) => {
  const style = {
    ...file,
    key: "css",
    name: "style.css",
    reference: "report/style.css",
    href: "/css",
    kind: "text" as const,
  };
  const image = {
    ...file,
    key: "image",
    name: "image.png",
    reference: "report/image.png",
    href: "/image",
    kind: "image" as const,
  };
  const fetched: string[] = [];
  context.mock.method(globalThis, "fetch", async (input: string) => {
    fetched.push(input);
    return new Response(
      input === "/css"
        ? ".a{background:url(image.png)}.b{background:url(image.png)}"
        : new Uint8Array([137, 80, 78, 71]),
    );
  });
  let count = 0;
  const output = await prepareHtml(
    '<link rel="stylesheet" href="style.css"><img src="image.png">',
    file,
    [file, style, image],
    false,
    new AbortController().signal,
    () => `blob:preview-${++count}`,
  );
  assert.deepEqual(fetched, ["/css", "/image"]);
  assert.ok(output.includes("<style>"));
  assert.ok(output.includes('src="data:image/png;base64,'));
});

test("HTML keeps inline interactivity and loads retained scripts without a browser service", async (context) => {
  const script = {
    ...file,
    key: "script",
    name: "app.js",
    reference: "report/app.js",
    href: "/script",
    kind: "text" as const,
  };
  context.mock.method(
    globalThis,
    "fetch",
    async () => new Response("document.body.dataset.ready='yes';"),
  );
  const warnings: string[] = [];
  const output = await prepareHtml(
    '<button onclick="this.textContent=\'Done\'">Run</button><script src="app.js"></script><img src="missing.png">',
    file,
    [file, script],
    true,
    new AbortController().signal,
    () => "blob:asset",
    (message) => warnings.push(message),
    "fixture-channel",
  );
  assert.ok(output.includes('onclick="this.textContent='));
  assert.ok(output.includes('src="data:text/javascript;base64,'));
  assert.ok(output.includes("kyler-preview-storage"));
  assert.ok(warnings.some((message) => message.includes("missing.png")));
});

test("CDN support is limited to explicit public asset hosts", async () => {
  const { previewCdn } = await import("../lib/kyler/artifact-preview.ts");
  assert.ok(previewCdn("https://cdn.jsdelivr.net/npm/chart.js"));
  for (const value of [
    "http://cdn.jsdelivr.net/a.js",
    "https://cdn.jsdelivr.net.evil.test/a.js",
    "https://user:key@cdn.jsdelivr.net/a.js",
    "http://localhost:30001/v1/info",
  ])
    assert.equal(previewCdn(value), null);
});

test("retained JSON data is embedded for offline fetch without exposing another execution", async (context) => {
  const data = {
    ...file,
    key: "data",
    name: "data.json",
    reference: "report/data.json",
    href: "/data",
    kind: "text" as const,
    resource: true,
  };
  const other = { ...data, key: "other", execution: "other", href: "/forbidden" };
  context.mock.method(globalThis, "fetch", async (input: string) => {
    assert.equal(input, "/data");
    return new Response('{"value":"</script><script>untrusted</script>"}');
  });
  const html = await prepareHtml(
    "<h1>Report</h1>",
    file,
    [file, data, other],
    true,
    new AbortController().signal,
    () => "blob:unused",
    () => undefined,
    "data-channel",
  );
  assert.ok(html.includes("window.fetch="));
  assert.ok(html.includes("\\u003c/script>"));
  assert.ok(!html.includes("</script><script>untrusted"));
});

test("file navigation filters resources and searches names and display paths", () => {
  const resource = { ...file, key: "resource", resource: true };
  const missing = { ...file, key: "missing", available: false };
  assert.deepEqual(displayPreviewFiles([resource, file], "REPORT"), [file]);
  assert.deepEqual(displayPreviewFiles([file], "absent"), []);
  assert.equal(selectedPreviewFile([resource, missing, file], "resource"), file);
  assert.equal(selectedPreviewFile([resource, missing, file], "missing"), missing);
  assert.equal(selectedPreviewFile([resource], null), undefined);
  const absolute = { ...file, reference: "/private/session/index.html" };
  assert.equal(previewPath(absolute), "index.html");
  assert.equal(absolute.href, file.href);
});

test("only explicit primary deliverables open automatically", () => {
  const secondary = { ...file, key: "secondary", primary: false };
  const legacy = { ...file, key: "legacy", primary: undefined };
  assert.equal(
    autoPreview([secondary, legacy], new Set(), "s", initialBrowserState),
    initialBrowserState,
  );
  assert.equal(
    autoPreview([secondary, file], new Set(), "s", initialBrowserState).selected,
    file.key,
  );
  assert.equal(
    autoPreview([{ ...file, available: false }, secondary], new Set(), "s", initialBrowserState),
    initialBrowserState,
  );
});
