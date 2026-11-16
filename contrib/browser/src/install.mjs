import { constants } from 'node:fs';
import { access, mkdir, copyFile, lstat, realpath, rm, symlink, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { delimiter, dirname, join } from 'node:path';
import { homedir } from 'node:os';

export async function linkTerminalBrowser(binary, binDir = join(homedir(), '.local', 'bin'), pathValue = process.env.PATH ?? '') {
  const source = await realpath(binary).catch(error => {
    if (error.code === 'ENOENT') throw new Error(`terminal-browser was not found at ${binary}`);
    throw error;
  });
  await access(source, constants.X_OK).catch(error => {
    if (error.code === 'EACCES') throw new Error(`terminal-browser is not executable at ${source}`);
    throw error;
  });
  await mkdir(binDir, { recursive: true });
  const command = join(binDir, 'terminal-browser');
  let status = 'unchanged';
  try {
    const existing = await lstat(command);
    if (!existing.isSymbolicLink()) throw new Error(`${command} already exists and is not a symbolic link; it was not replaced`);
    const target = await realpath(command).catch(() => null);
    if (target !== source) throw new Error(`${command} already points elsewhere; it was not replaced`);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    await symlink(source, command);
    status = 'created';
  }
  return { status, command, binary: source, onPath: pathValue.split(delimiter).includes(binDir) };
}

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
