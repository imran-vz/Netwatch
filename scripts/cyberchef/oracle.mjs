// Bake JSON cases with CyberChef: node oracle.mjs cases.json -> [{output} | {error} | {crash}]
import "./tests/lib/wasmFetchPolyfill.mjs";
import Chef from "./src/core/Chef.mjs";
import { readFileSync } from "node:fs";
const cases = JSON.parse(readFileSync(process.argv[2], "utf8"));
const out = [];
for (const c of cases) {
  try {
    const r = await new Chef().bake(c.input, c.recipe, { returnType: "string" });
    out.push(r.error ? { error: r.error.displayStr } : { output: r.result });
  } catch (e) {
    out.push({ crash: String(e.message ?? e) });
  }
}
process.stdout.write(JSON.stringify(out));
process.exit(0);
