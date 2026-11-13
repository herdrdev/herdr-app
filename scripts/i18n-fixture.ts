// English-only translation double for the existing lightweight mod harnesses.
// Production Fluent parsing, formatting and language selection are tested in Rust.
import { readdirSync, readFileSync } from "node:fs";

export function translatorFor(code = "en") {
const directory = new URL(`../defaults/locales/${code}/`, import.meta.url);
const messages = new Map<string, string>();
for (const file of readdirSync(directory)) {
  for (const match of readFileSync(new URL(file, directory), "utf8").matchAll(/^([\w-]+) = (.*(?:\n[ \t]+.*)*)/gm)) {
    messages.set(match[1], match[2]);
  }
}

return function translate(key: string, params: Record<string, string | number> = {}) {
  let value = messages.get(key);
  if (value === undefined) throw new Error(`Missing English interface message: ${key}`);
  if (value.includes("->")) {
    const variable = value.match(/\$([\w-]+)\s*->/)?.[1] ?? "count";
    const count = Number(params[variable]);
    const variant = count === 1 ? "one" : "other";
    value = value.match(new RegExp(`\\[${variant}\\] ([^\\n]*)`))?.[1] ?? value;
  }
  return value.replace(/\{ \$([\w-]+) \}/g, (_, name) => String(params[name] ?? `{ $${name} }`));
}
}

export const tr = translatorFor();

// new Function harnesses deliberately run outside module scope.
Object.assign(globalThis, { tr });
