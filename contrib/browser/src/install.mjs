import { mkdir, copyFile, writeFile, rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

export async function installMod(home, name = 'browser-launcher', binary = 'terminal-browser') {
  if (!/^[a-z0-9][a-z0-9-]*$/.test(name)) throw new Error('Mod names may contain lowercase letters, numbers and hyphens');
  const packageRoot = fileURLToPath(new URL('../', import.meta.url));
  const destination = join(home, 'mods', name);
  await mkdir(dirname(destination), { recursive: true });
  await mkdir(destination);
  try {
    for (const file of ['main.js', 'mod.json']) await copyFile(join(packageRoot, 'mod', file), join(destination, file));
    const command = [process.execPath, join(packageRoot, 'src', 'cli.mjs'), '--herdr-home', home, '--binary', binary];
    await writeFile(join(destination, 'config.js'), `export const command = ${JSON.stringify(command)};\n`);
  } catch (error) { await rm(destination, { recursive: true, force: true }); throw error; }
  return { name, path: destination, command: `herdr-app do mod.open --name ${name}` };
}
