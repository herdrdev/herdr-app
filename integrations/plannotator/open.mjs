#!/usr/bin/env node
import { execFile } from 'node:child_process';
import { access } from 'node:fs/promises';
import { homedir, release } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { requestBrowser } from './ipc.mjs';

const execute = promisify(execFile);
const wsl = process.platform === 'linux' && /microsoft/i.test(release());
const exists = async file => access(file).then(() => true, () => false);

export function findOriginPane(snapshot, terminal) {
    const spaceId = terminal.labels?.['herdr.space'];
    if (!spaceId || !terminal.address) return undefined;
    const matches = [];
    for (const space of snapshot.spaces ?? []) {
        if (space.id !== spaceId && !space.id.endsWith(`/${spaceId}`)) continue;
        for (const tab of space.tabs ?? []) {
            for (const item of tab.terminals ?? []) {
                if ((item.terminal === terminal.address || item.terminal?.endsWith(`/${terminal.address}`)) && typeof item.pane === 'string') matches.push(item.pane);
            }
        }
    }
    return matches.length === 1 ? matches[0] : undefined;
}

export async function runJson(command, args) {
    const { stdout } = await execute(command, args, { timeout: 15000, maxBuffer: 4 * 1024 * 1024, windowsHide: true });
    const value = JSON.parse(stdout);
    if (value.error) throw new Error(value.error);
    return value;
}

async function powershell() {
    if (process.env.HERDR_WINDOWS_POWERSHELL) return process.env.HERDR_WINDOWS_POWERSHELL;
    if (wsl) {
        const absolute = '/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe';
        if (await exists(absolute)) return absolute;
    }
    return 'powershell.exe';
}

export async function resolveApp() {
    if (process.env.HERDR_APP_BIN) return process.env.HERDR_APP_BIN;
    if (process.platform === 'win32') return path.join(process.env.LOCALAPPDATA || path.join(homedir(), 'AppData', 'Local'), 'Programs', 'herdr-app', 'herdr-app.exe');
    if (wsl) {
        const { stdout } = await execute(await powershell(), ['-NoProfile', '-NonInteractive', '-Command', '[Environment]::GetFolderPath("LocalApplicationData")'], { timeout: 10000, windowsHide: true });
        const windows = path.win32.join(stdout.trim(), 'Programs', 'herdr-app', 'herdr-app.exe');
        return (await execute('wslpath', ['-u', windows], { timeout: 5000 })).stdout.trim();
    }
    return 'herdr-app';
}

export async function openExternal(url) {
    if (process.platform === 'win32' || wsl) {
        const env = { ...process.env, HERDR_PLANNOTATOR_URL: url };
        if (wsl) env.WSLENV = [env.WSLENV, 'HERDR_PLANNOTATOR_URL'].filter(Boolean).join(':');
        await execute(await powershell(), ['-NoProfile', '-NonInteractive', '-Command', 'Set-Location $env:SystemRoot; Start-Process -FilePath $env:HERDR_PLANNOTATOR_URL -ErrorAction Stop'], { env, timeout: 15000, windowsHide: true });
    } else {
        await execute(process.platform === 'darwin' ? 'open' : 'xdg-open', [url], { timeout: 15000 });
    }
}

export async function openReview({ url, id }, isClosed = () => false) {
    if (isClosed()) return { kind: 'closed' };
    const parsed = new URL(url);
    if (!['http:', 'https:'].includes(parsed.protocol) || parsed.username || parsed.password) throw new Error('Expected an HTTP(S) URL without credentials');
    const session = process.env.HERDR_PLANNOTATOR_SESSION_ID;
    const terminalId = process.env.HC_TERMINAL_ID;
    if (!session || !terminalId || process.env.HERDR_PLANNOTATOR_MODE === 'external') {
        await openExternal(url);
        return { kind: 'external' };
    }
    try {
        const app = await resolveApp();
        const api = await runJson(app, ['--browser-api']);
        if (api.version !== 1 || api.module !== 'herdr-browser') throw new Error('The installed Herdr App has no native browser API');
        const [terminal, snapshot] = await Promise.all([
            runJson(process.env.HC_BIN || 'hc', ['get', terminalId, '--json']),
            runJson(app, ['ls']),
        ]);
        const origin = findOriginPane(snapshot, terminal);
        if (!origin) throw new Error('The requesting terminal has no unambiguous pane in this Herdr App');
        if (isClosed()) return { kind: 'closed' };
        const result = await runJson(app, ['do', 'pane.split', JSON.stringify({ direction: 'right', pane: origin, mod: 'plannotator-browser', props: { url, session, origin: terminalId, review: id } })]);
        if (typeof result.pane !== 'string') throw new Error('Herdr did not return the new browser pane');
        return { kind: 'embedded', app, pane: result.pane };
    } catch (error) {
        if (isClosed()) return { kind: 'closed' };
        // A missing fork/mod/app must never strand the review without a browser.
        console.error(`Herdr browser unavailable: ${error.message}. Opening the system browser.`);
        await openExternal(url);
        return { kind: 'external' };
    }
}

export async function closeReview(receipt) {
    if (receipt.kind === 'embedded') await runJson(receipt.app, ['do', 'pane.close', JSON.stringify({ pane: receipt.pane })]);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    const url = process.argv[2];
    const result = process.env.HERDR_PLANNOTATOR_SOCKET ? await requestBrowser(url) : { kind: 'external' };
    if (result.kind === 'external') await openExternal(url);
    console.log(JSON.stringify(result));
    if (result.kind === 'error') process.exitCode = 1;
}
