import { register } from "node:module";
import { readdirSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
register("./hooks.mjs", import.meta.url);
const [dir, out] = process.argv.slice(2);
globalThis.__tests = [];
const failed = [];
for (const f of readdirSync(dir).filter(f => f.endsWith(".mjs")).sort()) {
  globalThis.__file = f;
  try { await import(pathToFileURL(`${dir}/${f}`).href); } catch (e) { failed.push(`${f}: ${e.message.split("\n")[0]}`); }
}
const lone = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/;
const vectors = [];
let skipped = 0;
for (const t of globalThis.__tests) {
  if (typeof t.input !== "string" || !Array.isArray(t.recipeConfig) || (t.expectedOutput === undefined && !t.expectedMatch)) { skipped++; continue; }
  const json = JSON.stringify([t.input, t.expectedOutput ?? "", t.recipeConfig]);
  if (lone.test(t.input) || lone.test(t.expectedOutput ?? "")) { skipped++; continue; }
  const v = { file: t.file, name: t.name, input: t.input, recipe: t.recipeConfig };
  if (t.expectedOutput !== undefined) v.expected = t.expectedOutput;
  if (t.expectedMatch) v.expectedMatch = t.expectedMatch.source;
  if (t.expectedError) v.expectedError = true;
  vectors.push(v);
}
writeFileSync(out, JSON.stringify(vectors, null, 1));
console.log(`${vectors.length} vectors, ${skipped} skipped; files failed: ${failed.length}`);
for (const f of failed) console.log("  " + f);
