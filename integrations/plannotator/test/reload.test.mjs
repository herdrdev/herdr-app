import assert from 'node:assert/strict';
import test from 'node:test';
import { copyFile, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createAgentSession, createEventBus, DefaultResourceLoader, SessionManager, SettingsManager } from '@earendil-works/pi-coding-agent';
import { requestBrowser } from '../ipc.mjs';

const integration = fileURLToPath(new URL('..', import.meta.url));
const browserModule = version => `
export async function openReview() { return { kind: 'embedded', pane: ${JSON.stringify(version)} }; }
export async function closeReview() {}
export async function resolveApp() { return ''; }
export async function runJson() { return {}; }
`;

test('Pi reload uses the updated browser hook when its own hook was already configured', { timeout: 30000 }, async () => {
  const root = await mkdtemp(path.join(tmpdir(), 'herdr-reload-test-'));
  const keys = ['PLANNOTATOR_BROWSER', 'PLANNOTATOR_REMOTE', 'HERDR_PLANNOTATOR_SESSION_ID', 'HERDR_PLANNOTATOR_NODE', 'HERDR_PLANNOTATOR_SOCKET', 'HERDR_PLANNOTATOR_TOKEN', 'HERDR_PLANNOTATOR_MODE', 'HC_TERMINAL_ID'];
  const prior = new Map(keys.map(key => [key, process.env[key]]));
  let session;
  try {
    for (const file of ['index.ts', 'router.mjs', 'ipc.mjs', 'fresh-import.mjs']) await copyFile(path.join(integration, file), path.join(root, file));
    const modulePath = path.join(root, 'open.mjs');
    await writeFile(modulePath, browserModule('first'));
    for (const key of keys) delete process.env[key];
    process.env.PLANNOTATOR_BROWSER = modulePath;
    process.env.HERDR_PLANNOTATOR_SOCKET = path.join(root, 'previous.sock');
    const bus = createEventBus();
    const settings = SettingsManager.inMemory();
    const loader = new DefaultResourceLoader({ cwd: root, agentDir: root, settingsManager: settings, eventBus: bus, additionalExtensionPaths: [path.join(root, 'index.ts')], noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true });
    await loader.reload();
    ({ session } = await createAgentSession({ cwd: root, agentDir: root, resourceLoader: loader, settingsManager: settings, sessionManager: SessionManager.inMemory() }));
    await session.bindExtensions({ onError: error => { throw error; } });
    const request = async (id) => {
      const url = `http://localhost:12345/${id}`;
      bus.emit('plannotator:session-opened', { id, piSessionId: session.sessionId, url });
      return requestBrowser(url);
    };
    assert.equal((await request('pn-first')).pane, 'first');
    await writeFile(modulePath, browserModule('second'));
    await session.reload();
    assert.equal((await request('pn-second')).pane, 'second');
  } finally {
    if (session) {
      await session.extensionRunner.emit({ type: 'session_shutdown', reason: 'quit' });
      session.dispose();
    }
    for (const [key, value] of prior) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    await rm(root, { recursive: true, force: true });
  }
});
