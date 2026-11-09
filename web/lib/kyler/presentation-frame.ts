import pptxPackage from "@vue-office/pptx/package.json" with { type: "json" };
import { spreadsheetAssets } from "./spreadsheet-frame.ts";

export const presentationAssets = [
  ...spreadsheetAssets.slice(0, 2),
  `/pptx-preview/vue-office-pptx-${pptxPackage.version}.js`,
];

/** Self-contained: executes inside an opaque-origin frame, never in the host document. */
function installPresentationFrame() {
  const channel = document.body.dataset.channel;
  const send = (status: string, detail: Record<string, number> = {}) =>
    parent.postMessage(Object.assign({ type: "kyler-presentation", channel, status }, detail), "*");
  const runtime = window as unknown as {
    Vue: {
      createApp(options: { render: () => unknown }): {
        mount: (element: HTMLElement) => void;
        config: { errorHandler: () => void };
      };
      h(component: unknown, props: Record<string, unknown>): unknown;
    };
    "vue-office-pptx": unknown;
  };
  const deck = document.getElementById("deck")!;
  const surface = document.getElementById("surface")!;
  const viewport = document.getElementById("viewport")!;
  let slides: HTMLElement[] = [];
  let page = 1;
  let zoom: number | null = null;
  let opened = false;
  const layout = () => {
    const slide = slides[page - 1];
    if (!slide) return;
    slides.forEach((item, index) => {
      item.style.display = index === page - 1 ? "block" : "none";
    });
    const width = slide.offsetWidth || 960;
    const height = slide.offsetHeight || 540;
    const fit = Math.min(
      (viewport.clientWidth - 32) / width,
      (viewport.clientHeight - 32) / height,
    );
    const scale = Math.max(0.1, Math.min(4, zoom ?? fit));
    const left = Math.max(16, (viewport.clientWidth - width * scale) / 2);
    const top = Math.max(16, (viewport.clientHeight - height * scale) / 2);
    Object.assign(surface.style, {
      width: `${Math.max(viewport.clientWidth, width * scale + 32)}px`,
      height: `${Math.max(viewport.clientHeight, height * scale + 32)}px`,
    });
    Object.assign(deck.style, {
      width: `${width}px`,
      height: `${height}px`,
      left: `${left}px`,
      top: `${top}px`,
      transform: `scale(${scale})`,
    });
    send("position", { page, count: slides.length, zoom: scale });
  };
  window.addEventListener("resize", layout);
  if (!runtime.Vue || !runtime["vue-office-pptx"]) {
    send("error");
    return;
  }
  window.addEventListener("message", (event: MessageEvent) => {
    if (event.source !== parent || event.data?.channel !== channel) return;
    const data = event.data;
    if (data.type === "kyler-presentation-control" && slides.length) {
      if (Number.isInteger(data.page) && data.page >= 1 && data.page <= slides.length) {
        page = data.page;
        viewport.scrollTop = 0;
        viewport.scrollLeft = 0;
      }
      if (data.fit === true) zoom = null;
      else if (Number.isFinite(data.zoom) && data.zoom >= 0.1 && data.zoom <= 4) zoom = data.zoom;
      layout();
      return;
    }
    if (data.type !== "kyler-presentation-open" || opened || !(data.buffer instanceof ArrayBuffer))
      return;
    opened = true;
    try {
      const app = runtime.Vue.createApp({
        render: () =>
          runtime.Vue.h(runtime["vue-office-pptx"], {
            src: data.buffer,
            options: { width: 960, height: 540 },
            onRendered: () => {
              slides = [...deck.querySelectorAll<HTMLElement>(".pptx-preview-slide-wrapper")];
              if (!slides.length || slides.length > 100) {
                send("error");
                return;
              }
              const wrapper = deck.querySelector<HTMLElement>(".pptx-preview-wrapper");
              if (wrapper)
                Object.assign(wrapper.style, {
                  height: "auto",
                  overflow: "visible",
                  background: "transparent",
                });
              slides.forEach((slide) => {
                slide.style.margin = "0";
              });
              send("rendered", { page, count: slides.length });
              layout();
            },
            onError: () => send("error"),
          }),
      });
      app.config.errorHandler = () => send("error");
      app.mount(deck);
    } catch {
      send("error");
    }
  });
  send("ready");
}

export function presentationFrameDocument(channel: string, scripts: string[]) {
  if (
    !/^[a-zA-Z0-9-]+$/.test(channel) ||
    scripts.length !== 3 ||
    scripts.some((source) => source.length > 8 * 1024 * 1024)
  )
    throw new Error("Invalid presentation frame resources.");
  const script = (source: string) => source.replace(/<\/script/gi, "<\\/script");
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'nonce-${channel}'; style-src 'unsafe-inline'; img-src blob: data:; font-src blob: data:; connect-src blob:; base-uri 'none'; form-action 'none'"><style>html,body,#viewport{height:100%;width:100%;margin:0}body{overflow:hidden;background:#e5e7eb}#viewport{overflow:auto;position:relative}#surface{position:relative}#deck{position:absolute;transform-origin:0 0}*{box-sizing:content-box}</style><script nonce="${channel}">window.process={env:{NODE_ENV:"production"}};</script>${scripts.map((source) => `<script nonce="${channel}">${script(source)}</script>`).join("")}</head><body data-channel="${channel}"><div id="viewport"><div id="surface"><div id="deck"></div></div></div><script nonce="${channel}">(${installPresentationFrame.toString()})();</script></body></html>`;
}
