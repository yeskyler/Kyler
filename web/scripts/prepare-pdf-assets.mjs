import { cp, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
const target = new URL("../public/pdfjs/", import.meta.url);
await mkdir(target, { recursive: true });
for (const entry of ["cmaps", "standard_fonts", "wasm", "LICENSE", "web/images"]) {
  await cp(
    fileURLToPath(new URL(`../node_modules/pdfjs-dist/${entry}`, import.meta.url)),
    fileURLToPath(new URL(entry === "web/images" ? "images" : entry, target)),
    { recursive: true },
  );
}
