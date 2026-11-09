import { cp, mkdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
const destination = new URL("../public/excel-preview/", import.meta.url);
await mkdir(destination, { recursive: true });
for (const [name, source, extension] of [
  ["vue", "dist/vue.global.prod.js", "js"],
  ["vue-demi", "lib/index.iife.js", "js"],
  ["@vue-office/excel", "lib/v3/index.js", "js"],
  ["@vue-office/excel", "lib/index.css", "css"],
]) {
  const root = new URL(`../node_modules/${name}/`, import.meta.url);
  const { version } = JSON.parse(await readFile(new URL("package.json", root), "utf8"));
  const filename = `${name.replaceAll("/", "-").replaceAll("@", "")}-${version}.${extension}`;
  await cp(fileURLToPath(new URL(source, root)), fileURLToPath(new URL(filename, destination)));
}
await writeFile(
  new URL("NOTICE.txt", destination),
  "Excel preview uses @vue-office/excel, Vue, and vue-demi (MIT).\n" +
    "Upstream: https://github.com/501351981/vue-office\nhttps://github.com/vuejs/core\nhttps://github.com/vueuse/vue-demi\n" +
    "Copyright and third-party license notices in the distributed JavaScript are retained.\n",
);
for (const name of ["vue", "vue-demi"]) {
  await cp(
    new URL(`../node_modules/${name}/LICENSE`, import.meta.url),
    new URL(`${name}-LICENSE.txt`, destination),
  );
}
