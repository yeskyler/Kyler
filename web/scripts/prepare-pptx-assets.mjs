import { cp, mkdir, readFile, writeFile } from "node:fs/promises";
const root = new URL("../node_modules/@vue-office/pptx/", import.meta.url);
const destination = new URL("../public/pptx-preview/", import.meta.url);
const { version } = JSON.parse(await readFile(new URL("package.json", root), "utf8"));
await mkdir(destination, { recursive: true });
await cp(new URL("lib/v3/index.js", root), new URL(`vue-office-pptx-${version}.js`, destination));
await cp(new URL("README.md", root), new URL("UPSTREAM-README.md", destination));
await writeFile(
  new URL("NOTICE.txt", destination),
  "PPTX preview uses @vue-office/pptx.\nUpstream: https://github.com/501351981/vue-office\n" +
    "Bundled engine: https://github.com/501351981/pptx-preview\n" +
    "Upstream copyright and license notices in the distributed JavaScript are retained.\n" +
    "See UPSTREAM-README.md for component and engine distribution information.\n",
);
