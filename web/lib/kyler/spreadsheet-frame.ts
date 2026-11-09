import excelPackage from "@vue-office/excel/package.json" with { type: "json" };
import vuePackage from "vue/package.json" with { type: "json" };
import demiPackage from "vue-demi/package.json" with { type: "json" };

export const spreadsheetAssets = [
  `/excel-preview/vue-${vuePackage.version}.js`,
  `/excel-preview/vue-demi-${demiPackage.version}.js`,
  `/excel-preview/vue-office-excel-${excelPackage.version}.js`,
  `/excel-preview/vue-office-excel-${excelPackage.version}.css`,
];

/** Self-contained: this function runs only in an opaque-origin sandbox iframe. */
function installSpreadsheetFrame() {
  const channel = document.body.dataset.channel;
  const send = (status: string) =>
    parent.postMessage({ type: "kyler-spreadsheet", channel, status }, "*");
  const runtime = window as unknown as {
    Vue: {
      createApp(options: { render: () => unknown }): {
        mount: (element: HTMLElement) => void;
        config: { errorHandler: () => void };
      };
      h(component: unknown, props: Record<string, unknown>): unknown;
    };
    "vue-office-excel": unknown;
  };
  if (!runtime.Vue || !runtime["vue-office-excel"]) {
    send("error");
    return;
  }
  let opened = false;
  window.addEventListener("message", (event: MessageEvent) => {
    if (
      event.source !== parent ||
      event.data?.channel !== channel ||
      event.data?.type !== "kyler-spreadsheet-open" ||
      opened
    )
      return;
    if (!(event.data.buffer instanceof ArrayBuffer)) return;
    opened = true;
    try {
      const app = runtime.Vue.createApp({
        render: () =>
          runtime.Vue.h(runtime["vue-office-excel"], {
            src: event.data.buffer,
            options: { minColLength: 0, minRowLength: 0, showContextmenu: false },
            onRendered: () => send("rendered"),
            onError: () => send("error"),
          }),
      });
      app.config.errorHandler = () => send("error");
      app.mount(document.getElementById("workbook")!);
    } catch {
      send("error");
    }
  });
  send("ready");
}

export function spreadsheetFrameDocument(channel: string, scripts: string[], stylesheet: string) {
  if (
    !/^[a-zA-Z0-9-]+$/.test(channel) ||
    scripts.length !== 3 ||
    [...scripts, stylesheet].some((source) => source.length > 8 * 1024 * 1024)
  )
    throw new Error("Invalid spreadsheet frame resources.");
  const script = (source: string) => source.replace(/<\/script/gi, "<\\/script");
  const style = stylesheet.replace(/<\/style/gi, "<\\/style");
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'nonce-${channel}'; style-src 'unsafe-inline'; img-src blob: data:; font-src data:; connect-src blob:; base-uri 'none'; form-action 'none'"><style>${style}</style><style>html,body,#workbook{height:100%;width:100%;margin:0;overflow:hidden;background:white;color:#111}*{box-sizing:content-box}</style>${scripts.map((source) => `<script nonce="${channel}">${script(source)}</script>`).join("")}</head><body data-channel="${channel}"><div id="workbook"></div><script nonce="${channel}">(${installSpreadsheetFrame.toString()})();</script></body></html>`;
}
