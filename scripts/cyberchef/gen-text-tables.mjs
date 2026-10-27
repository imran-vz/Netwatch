// Generates text/codepages.rs and text/entities.rs for utilify-chef's text group
// from the codepage npm package and CyberChef's generated HTML entity tables.
//
//   node gen-text-tables.mjs <CyberChef checkout with deps, see README> <out dir>
//   then `cargo fmt`.
import { createRequire } from "node:module";
import { writeFileSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
const [chef, outDir] = process.argv.slice(2);
const require = createRequire(import.meta.url);
const cpt = require(`${chef}/node_modules/codepage/cptable.js`);
const { HTML_ENTITY_LOOKUP, HTML_ENTITY_REVERSE_LOOKUP } =
    await import(pathToFileURL(`${chef}/src/core/lib/HTMLEntities.mjs`).href);

const src = readFileSync(`${chef}/src/core/lib/ChrEnc.mjs`, "utf8");
const pages = [...src.matchAll(/^\s+"(.+)": (\d+),$/gm)].map(m => +m[2]);

const unsafe = /[\p{C}\p{Z}\p{M}"\\]/u;
function lit(units) {
    let s = "";
    for (const u of units) {
        const c = String.fromCharCode(u);
        if (u < 0x7f && u >= 0x20 && c !== '"' && c !== "\\") s += c;
        else if (u < 0x80 || unsafe.test(c)) s += `\\u{${u.toString(16)}}`;
        else s += c;
    }
    return `"${s}"`;
}

let rs = `//! Code page tables for "Encode text" and "Decode text", generated from the
//! \`codepage\` npm package (v${cpt.version}) that CyberChef uses. Do not edit.
//!
//! A single-byte page is its 256 characters in byte order. A double-byte page
//! is a list of runs: the first code of the run and the characters of the
//! consecutive codes from there; codes in no run are unmapped.

pub(super) enum Table {
    Sbcs(&'static str),
    Dbcs(&'static [(u16, &'static str)]),
}

/// Every table-driven page CyberChef lists, by code page number.
pub(super) static TABLES: &[(u32, Table)] = &[
`;
for (const p of pages) {
    const C = cpt[p];
    if (!C) continue;
    const D = C.dec;
    if (D.length <= 256) {
        if (D.length !== 256) throw new Error("short sbcs " + p);
        rs += `    (${p}, Table::Sbcs(${lit(D.map(c => c.charCodeAt(0)))})),\n`;
        continue;
    }
    rs += `    (\n        ${p},\n        Table::Dbcs(&[\n`;
    let i = 0;
    while (i < D.length) {
        if (!D[i]) { i++; continue; }
        const start = i, run = [];
        while (i < D.length && D[i] && run.length < 256) { run.push(D[i].charCodeAt(0)); i++; }
        rs += `            (0x${start.toString(16).padStart(4, "0")}, ${lit(run)}),\n`;
    }
    rs += `        ]),\n    ),\n`;
}
rs += "];\n";
writeFileSync(outDir + "/codepages.rs", rs);

let es = `//! HTML entity tables, generated from CyberChef's
//! src/core/lib/HTMLEntities.mjs (the WHATWG named character references plus
//! CyberChef's overrides). Do not edit.

/// Code point to its canonical entity name, sorted by code point; used to encode.
pub(super) static ENTITY_NAMES: &[(u32, &str)] = &[
`;
const cps = Object.keys(HTML_ENTITY_LOOKUP).map(Number).sort((a, b) => a - b);
for (const c of cps) es += `    (${c}, "${HTML_ENTITY_LOOKUP[c]}"),\n`;
es += `];

/// Entity name (canonical or legacy alias) to code point, sorted by name; used
/// to decode.
pub(super) static ENTITY_CODES: &[(&str, u32)] = &[
`;
const names = Object.keys(HTML_ENTITY_REVERSE_LOOKUP).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
for (const n of names) {
    if (!/^[A-Za-z0-9]+$/.test(n)) throw new Error("odd name " + n);
    es += `    ("${n}", ${HTML_ENTITY_REVERSE_LOOKUP[n]}),\n`;
}
es += "];\n";
writeFileSync(outDir + "/entities.rs", es);
console.log("pages", pages.length, "entities", cps.length, names.length);
