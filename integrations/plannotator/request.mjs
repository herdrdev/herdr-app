#!/usr/bin/env node
// Starts a Plannotator command in the Pi of a herdr-core terminal on this
// machine, for Herdr's tab bar: `request.mjs <terminal> <review|last> [hc]`.
// The terminal's `herdr.plannotator` label, which that Pi publishes, says
// how to reach it; it counts only while that Pi is the terminal's foreground.
import { execFileSync } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { COMMANDS, requestCommand } from './ipc.mjs';

export function reachable(terminal) {
    const label = JSON.parse(terminal?.labels?.['herdr.plannotator'] ?? 'null');
    return label && label.pid === terminal.foreground?.pid ? label : null;
}

// `hc`: herdr-core's CLI, by path when the caller's PATH lacks it (the label's `hc`).
async function main([terminal, command, hc = process.env.HC_BIN || 'hc']) {
    if (!terminal || !COMMANDS.includes(command)) throw new Error(`Usage: request.mjs <terminal> <${COMMANDS.join('|')}> [hc]`);
    const info = JSON.parse(execFileSync(hc, ['get', terminal, '--json'], { encoding: 'utf8', timeout: 5000 }));
    const label = reachable(info);
    if (!label) throw new Error('No Pi with the Herdr Plannotator integration runs in this terminal');
    const answer = await requestCommand(command, label);
    if (answer.kind !== 'started') throw new Error(answer.message || answer.code || 'The Pi session refused the request');
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
    main(process.argv.slice(2)).catch(error => { console.error(error instanceof Error ? error.message : String(error)); process.exit(1); });
}
