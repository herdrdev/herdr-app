import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The pane and terminal headers show a pane's folder; check their folder() on Unix and Windows paths.
const mods = ["pane", "terminals-pane", "terminals-bar"];

async function load(mod: string, homeDir: string) {
  const source = await readFile(new URL(`../defaults/mods/${mod}/main.js`, import.meta.url), "utf8");
  const body = source.match(/^function folder\(cwd\) \{[\s\S]*?^\}/m)?.[0];
  if (!body) throw new Error(`${mod}: folder() not found`);
  return new Function("home", `${body}\nreturn folder;`)(() => homeDir) as (cwd?: string) => string;
}

for (const mod of mods) {
  test(`${mod}: Unix paths keep their shape`, async () => {
    const folder = await load(mod, "/home/me");
    expect(folder("/home/me")).toBe("~");
    expect(folder("/home/me/projects/demo")).toBe("~/projects/demo");
    expect(folder("/home/Me/projects")).toBe("/home/Me/projects");
    expect(folder("/srv/app")).toBe("/srv/app");
    expect(folder("")).toBe("");
  });

  test(`${mod}: Windows paths use forward slashes and collapse the home folder`, async () => {
    const folder = await load(mod, "C:\\Users\\me");
    expect(folder("C:\\Users\\me")).toBe("~");
    expect(folder("C:\\Users\\me\\projects\\demo")).toBe("~/projects/demo");
    expect(folder("c:\\users\\me\\.config")).toBe("~/.config");
    expect(folder("D:\\code\\api")).toBe("D:/code/api");
    expect(folder("C:\\Users\\meX\\notes")).toBe("C:/Users/meX/notes");
  });
}
