export default { tests: globalThis.__tests ??= [], addTests(t) { for (const x of t) globalThis.__tests.push({ file: globalThis.__file, ...x }); }, addApiTests() {} };
