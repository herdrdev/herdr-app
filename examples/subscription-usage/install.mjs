// Optional personal mod installer. No app restart, settings or keybinding edits.
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const source = fileURLToPath(new URL("./mods/subscription-usage/", import.meta.url));
const files = ["main.js", "mod.json", "package.json", "query.mjs", "quota.js"];

export function appHome(env = process.env, home = homedir()) {
  return resolve(env.HERDR_APP_HOME || join(env.XDG_CONFIG_HOME || join(home, ".config"), "herdr-app"));
}

export async function install({ home = appHome(), name = "subscription-usage" } = {}) {
  if (!/^[a-z][a-z0-9-]{0,63}$/.test(name)) throw new Error("Use a mod name containing lowercase letters, digits and hyphens, starting with a letter");
  const target = join(resolve(home), "mods", name);
  // Read the complete package before touching the destination.
  const contents = await Promise.all(files.map(file => readFile(join(source, file), "utf8")));
  await mkdir(join(resolve(home), "mods"), { recursive: true });
  try { await mkdir(target); } catch (error) {
    if (error.code === "EEXIST") throw new Error(`Already exists: ${target}. Install under a different --name; existing files were preserved.`);
    throw error;
  }
  try {
    // Publish main.js last: Herdr discovers a mod only once its entry point exists.
    for (const file of [...files.slice(1), files[0]]) {
      const content = contents[files.indexOf(file)].replaceAll("/mods/subscription-usage/", `/mods/${name}/`);
      await writeFile(join(target, file), content, { flag: "wx" });
    }
  } catch (error) {
    throw new Error(`Installation incomplete at ${target}; inspect this new folder before retrying. ${error.code || "Write failed"}`);
  }
  return target;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const options = {};
    const args = process.argv.slice(2);
    for (let i = 0; i < args.length; i++) {
      if (!["--home", "--name"].includes(args[i]) || !args[i + 1] || args[i + 1].startsWith("--")) throw new Error("Usage: node install.mjs [--home APP_HOME] [--name subscription-usage]");
      options[args[i].slice(2)] = args[++i];
    }
    const target = await install(options);
    console.log(`Installed ${target}\nCheck: herdr-app check "${target}"\nOpen: herdr-app do mod.open --name ${options.name || "subscription-usage"}`);
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
