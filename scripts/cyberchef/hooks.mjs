// Redirect CyberChef's TestRegister import to a stub that records tests.
export async function resolve(specifier, context, next) {
  if (specifier.endsWith("TestRegister.mjs")) {
    return { url: new URL("./stub-register.mjs", import.meta.url).href, shortCircuit: true };
  }
  return next(specifier, context);
}
