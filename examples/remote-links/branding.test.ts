import { expect, test } from "bun:test";
import { createContext, runInContext } from "node:vm";
import { titleBrandingSource } from "./mod/branding";

function fixture(title: string) {
  let observe: (() => void) | undefined;
  const document = { title, head: {}, addEventListener() {} };
  const context = createContext({ document, MutationObserver: class {
    constructor(callback: () => void) { observe = callback; }
    observe() {}
  } });
  return { context, document, update: () => observe?.() };
}

test("preserves the page title after the remote-machine prefix", () => {
  const f = fixture("Dashboard");
  runInContext(titleBrandingSource("dev"), f.context);
  expect(f.document.title).toBe("Herdr · dev — Dashboard");
  f.document.title = "Reactive app title";
  f.update();
  expect(f.document.title).toBe("Herdr · dev — Reactive app title");
  f.update();
  expect(f.document.title).toBe("Herdr · dev — Reactive app title");
});
test("an untitled page still shows its machine", () => {
  const f = fixture("");
  runInContext(titleBrandingSource("remote-host"), f.context);
  expect(f.document.title).toBe("Herdr · remote-host");
});
test("repeated injection is idempotent and quotes machine names", () => {
  const f = fixture("Page");
  const machine = "dev');globalThis.pwned=true;//";
  runInContext(titleBrandingSource(machine), f.context);
  runInContext(titleBrandingSource(machine), f.context);
  expect(f.document.title).toBe("Herdr · " + machine + " — Page");
  expect(runInContext("globalThis.pwned", f.context)).toBeUndefined();
});
