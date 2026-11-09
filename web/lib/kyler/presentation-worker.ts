import { preparePresentation } from "./presentation";

self.onmessage = (event: MessageEvent<ArrayBuffer>) => {
  try {
    const bytes = preparePresentation(new Uint8Array(event.data));
    const buffer = bytes.slice().buffer;
    self.postMessage({ buffer }, { transfer: [buffer] });
  } catch (failure) {
    self.postMessage({
      error: failure instanceof Error ? failure.message : "Presentation preparation failed.",
    });
  }
};
